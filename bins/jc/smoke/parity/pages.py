"""Pages and features compared per pair. A path is None where that side has no such page."""

from __future__ import annotations

import json
import os
from pathlib import Path
from urllib.parse import quote

DOMAIN = "dev.joinedcontext.com"
OLD = {
    "ckan": f"https://opendata.{DOMAIN}",
    "assistant": f"https://opendata.{DOMAIN}",
    "grafana": f"https://{DOMAIN}",
    "gitea": f"https://{DOMAIN}",
    "keycloak": f"https://idm.{DOMAIN}",
    "pipelines": f"https://portal.{DOMAIN}",
    "portal": f"https://portal.{DOMAIN}",
}
OLD_KEYCLOAK = OLD["keycloak"]
# Production, the reference for the catalogue and the assistant: public pages only, no sign-in.
PROD = {"ckan": "https://opendata.smartcity.joinedcontext.com", "assistant": "https://opendata.smartcity.joinedcontext.com"}
PROJECT = "bbsk"
DATASET = "bbsk-podujatia"


# Dev's Gitea shows demo.editor no repository, so the old forge pages can come from a local Gitea of
# dev's version holding the export's repository files: PARITY_OLD_GITEA=<url>, PARITY_OLD_GITEA_PERSON=<file>.
OLD_GITEA_LOCAL = os.environ.get("PARITY_OLD_GITEA")


def base_of(pair: str, side: str, base: str) -> str:
    if side == "prod":
        return PROD[pair]
    if side == "old" and pair == "gitea" and OLD_GITEA_LOCAL:
        return OLD_GITEA_LOCAL
    return OLD[pair] if side == "old" else base


def old_path(pair: str, path: str) -> str:
    return path.removeprefix("/git") or "/" if pair == "gitea" and OLD_GITEA_LOCAL else path


def portal_base(side: str, base: str) -> str:
    return OLD["portal"] if side == "old" else base


def F(name, sel=None, old=None, new=None, texts=False, js=None, what=None):
    return {"name": name, "sel": sel, "old": old, "new": new, "texts": texts, "js": js, "what": what}


def J(name, js, what):
    """A content or layout check: a JS expression true in the page or one of its frames."""
    return F(name, js=js, what=what)


# What the images showed missing (supervisor 2026-11-08): content and layout, not only presence.
LAYOUT = [
    J("header nav in one row, no bullets", "(()=>{const l=[...document.querySelectorAll('.masthead .navbar-nav > li')];"
      "if(l.length<2) return false; const t=l.map(e=>Math.round(e.getBoundingClientRect().top));"
      "return t.every(x=>Math.abs(x-t[0])<4) && getComputedStyle(l[0]).listStyleType==='none'})()", "masthead nav items share one row"),
    J("skip link hidden until focused", "(()=>{const a=[...document.querySelectorAll('a')].find(e=>/Skip to|Preskočiť/.test(e.textContent));"
      "if(!a) return true; for(let e=a;e;e=e.parentElement){const r=e.getBoundingClientRect(), c=getComputedStyle(e);"
      "if(r.width<=1||r.height<=1||c.clip!=='auto'||c.visibility==='hidden'||c.display==='none') return true} return false})()",
      "skip link or an ancestor has no visible box"),
    J("logo text not cut", "(()=>{const e=document.querySelector('.masthead .logo, .masthead .jc-brand');"
      "return !!e && e.scrollWidth<=e.clientWidth+1})()", "brand scrollWidth <= clientWidth"),
    J("footer in columns", "(()=>{const c=[...document.querySelectorAll('footer .row > *')];"
      "return c.length>=2 && Math.abs(c[0].getBoundingClientRect().top-c[1].getBoundingClientRect().top)<4})()", "footer columns side by side"),
    F("assistant launcher button", "#jc-assistant summary, summary:has-text('Opýtajte sa asistenta'), button:has-text('Opýtajte sa asistenta')"),
    J("AI chips Claude / ChatGPT / Copilot", "document.querySelectorAll('.jc-assistants-link .jc-seg').length>=3"
      " && [...document.querySelectorAll('.jc-assistants-link .jc-seg')].every(e=>e.getBoundingClientRect().width>8)",
      "three drawn chips in .jc-assistants-link"),
]
DATASET_CONTENT = [
    J("Živé dáta grid with filters and paging", "!!document.querySelector('table, [role=grid]') && /Filter/.test(document.body.innerText)"
      " && /Strana|Nasledujúca|Next|Page /.test(document.body.innerText)", "a table, a Filter control and paging in one frame"),
    J("map drawn", "[...document.querySelectorAll('canvas')].some(c=>c.width>50&&c.height>50)", "a canvas of 50+ px"),
    J("Dátové API and Vložiť na web buttons", "/Dátové API/.test(document.body.innerText) && /Vložiť na web/.test(document.body.innerText)", "both button labels"),
    J("Živé dáta (API) curl box", "!!document.querySelector('#jc-live-title') && /curl -H/.test(document.body.innerText)", "#jc-live-title and a curl line"),
    F("Pripojte AI asistenta (MCP)", "#jc-mcp-title"),
]


def _visit(page, url):
    try:
        page.goto(url, wait_until="load", timeout=30000)
        page.wait_for_timeout(2000)
    except Exception as e:  # noqa: BLE001 - a failed visit leaves the names unresolved, the pages then say why
        print(f"visit {url}: {str(e)[:120]}")


def _get(page, url, headers=None):
    r = page.request.get(url, timeout=30000, headers=headers or {})
    return json.loads(r.text()) if r.ok else None


# --- CKAN (jc_theme) vs dataroom ---------------------------------------------------------------
CKAN_HOME = [
    F("search box", "form input[name=q]"),
    F("dataset count / stats", ".jc-count, .stats, .jc-stats"),
    F("organization links", "a[href*='/organization/']"),
    F("group / topic links", "a[href*='/group/'], #jc-topics-title"),
    F("language switch", "a[href*='/en/'], a[href^='/sk'], .jc-lang, [lang-switch], a[hreflang]"),
]
CKAN_LIST = [
    F("search box", "form input[name=q]"),
    F("result count", "h1:has-text('dataset'), h2:has-text('dataset'), .jc-count, .search-form h1, .search-form h2"),
    F("dataset items", "li.dataset-item, .dataset-item, article.jc-item"),
    F("facet: organization", "a[href*='organization=']"),
    F("facet: tags", "a[href*='tags=']"),
    F("facet: formats", "a[href*='res_format=']"),
    F("facet: license", "a[href*='license_id=']"),
    F("sort control", "select[name=sort]"),
    F("pagination", ".pagination, nav[aria-label*='aging'], a[href*='page=2']"),
]
CKAN_DATASET = [
    F("title h1", "h1"),
    F("live data explorer (data grid)", "#jc-explorer-title, .jc-explorer"),
    F("table in preview", ".jc-preview table, .jc-explorer table, jc-grid, .jc-grid, [role=grid]"),
    F("map", ".jc-map, .maplibregl-map, .leaflet-container, canvas"),
    F("resources list", "#jc-resources-title, .resource-item, .resource-list"),
    F("NGSI-LD API link", "a[href*='/ngsi-ld/v1']"),
    F("CSV download", "a[href*='file.csv']"),
    F("OGC features link", "a[href*='/ogc/']"),
    F("WebSocket address", "text=/\\/ws\\b/"),
    F("MCP / AI assistant section", "#jc-mcp-title"),
    F("data model section", "#jc-section-schema"),
    F("about / metadata table", "#jc-about-title, table.additional-info, .jc-about-list"),
    F("tags", "a[href*='tags=']"),
    F("license", "text=/[Ll]icen/"),
    F("organization link", "a[href*='/organization/']"),
]
CKAN_RESOURCE = [
    F("title h1", "h1"),
    F("download / go-to URL button", "a.resource-url-analytics, a[href*='file.csv'], a[href*='/ngsi-ld/v1'], .jc-resource-actions a"),
    F("preview / view iframe or table", "iframe, .jc-preview, table"),
    F("resource metadata table", "table"),
]
CKAN_API = [F("JSON body", "pre, body")]

CKAN_PAGES_ = [
    {"id": "home", "old": "/", "new": "/opendata/", "features": CKAN_HOME},
    {"id": "organizations", "old": "/organization/", "new": "/opendata/organization/",
     "features": [F("organization cards", "a[href*='/organization/']"), F("search box", "input[name=q]")]},
    {"id": "organization", "old": "/organization/{org}", "new": "/opendata/organization/{org}", "features": CKAN_LIST},
    {"id": "datasets", "old": "/dataset/", "new": "/opendata/dataset/", "features": CKAN_LIST},
    {"id": "datasets-search", "old": "/dataset/?q=podujatia", "new": "/opendata/dataset/?q=podujatia", "features": CKAN_LIST},
    {"id": "datasets-filter", "old": "/dataset/?organization=bbsk&res_format=CSV",
     "new": "/opendata/dataset/?organization=bbsk&res_format=CSV", "features": CKAN_LIST},
    {"id": "dataset", "old": "/dataset/{dataset}", "new": "/opendata/dataset/{dataset}", "features": CKAN_DATASET, "settle": 6000},
    {"id": "resource", "old": "/dataset/{dataset}/resource/{resource_old}",
     "new": "/opendata/dataset/{dataset}/resource/{resource_new}", "features": CKAN_RESOURCE, "settle": 5000},
    {"id": "groups", "old": "/group/", "new": "/opendata/group/", "features": [F("group items", "a[href*='/group/']")]},
    {"id": "dcat-feed", "old": "/catalog.ttl", "new": "/opendata/catalog.ttl", "features": [F("dcat:Dataset", "text=/dcat:Dataset/")]},
    {"id": "atom-feed", "old": "/feeds/dataset.atom", "new": "/opendata/feeds/dataset.atom", "features": [F("atom entry", "text=/entry|<title>/")]},
    {"id": "api-package-search", "old": "/api/3/action/package_search?rows=2", "new": "/opendata/api/3/action/package_search?rows=2",
     "features": CKAN_API},
    {"id": "api-package-show", "old": "/api/3/action/package_show?id={dataset}", "new": "/opendata/api/3/action/package_show?id={dataset}",
     "features": CKAN_API},
]

CKAN_PAGES = []
for _p in CKAN_PAGES_:
    _p = dict(_p, prod=_p["old"].replace("{resource_old}", "{resource_prod}"))  # production shows the same paths
    if not _p["id"].startswith(("api-", "dcat", "atom")):
        _p["features"] = _p["features"] + LAYOUT + (DATASET_CONTENT if _p["id"] == "dataset" else [])
    CKAN_PAGES.append(_p)

# --- AI assistant (production's assistant and the launcher on the catalogue pages) -----------------
ASSIST_LAUNCH = [LAYOUT[4], LAYOUT[5],
                 F("assistant dock frame", "iframe[src*='assistant'], iframe[src*='/widget']"),
                 J("question box in the dock", "!!document.querySelector('textarea, input[type=text]') && location.href.includes('widget')", "a text box inside the widget frame")]
ASSIST_PAGE = [F("question box", "textarea, input[type=text]"), F("send button", "button[type=submit], button:has-text('Odosla'), button:has-text('Send')"),
               J("example questions / intro", "document.body.innerText.length > 200", "an intro text"), F("sources / about link", "a[href]")]
ASSIST_PAGES = [
    {"id": "launcher-home", "prod": "/", "old": f"https://opendata.{DOMAIN}/", "new": "/opendata/", "features": ASSIST_LAUNCH},
    {"id": "launcher-open", "prod": "/", "old": f"https://opendata.{DOMAIN}/", "new": "/opendata/", "settle": 6000,
     "then": [{"click": "#jc-assistant summary, summary:has-text('Opýtajte sa asistenta')"}], "features": ASSIST_LAUNCH},
    {"id": "launcher-dataset", "prod": f"/dataset/{DATASET}", "old": f"https://opendata.{DOMAIN}/dataset/{DATASET}",
     "new": f"/opendata/dataset/{DATASET}", "features": ASSIST_LAUNCH},
    {"id": "assistants-guide", "prod": "/assistants", "old": f"https://opendata.{DOMAIN}/assistants", "new": "/opendata/assistants",
     "features": [F("client list", "h2, h3"), J("Claude / ChatGPT / Copilot", "['Claude','ChatGPT','Copilot'].every(w=>document.body.innerText.includes(w))", "all three names")]},
    {"id": "assistant-page", "prod": "https://assistant.smartcity.joinedcontext.com/d/data", "old": f"https://assistant.{DOMAIN}/d/data",
     "new": None, "why": "jc serves no assistant module: no public assistant page (assistant.<domain>/d/<deployment>)", "features": ASSIST_PAGE, "settle": 5000},
    {"id": "assistant-widget", "prod": "https://assistant.smartcity.joinedcontext.com/d/data/widget?embed=1&scheme=light",
     "old": f"https://assistant.{DOMAIN}/d/data/widget?embed=1&scheme=light", "new": None,
     "why": "jc serves no assistant module: no widget the catalogue launcher can open", "features": ASSIST_PAGE, "settle": 5000},
]

# --- Grafana vs daiya ----------------------------------------------------------------------------
GRAFANA_LIST = [F("dashboard rows", "a[href*='/d/']"), F("search input", "input[placeholder*='earch'], input[type=search]"),
                F("folder rows", "a[href*='/dashboards/f/']")]
GRAFANA_DASH = [F("panels", old="[data-viz-panel-key]", new="section[data-panel-type]"),
                F("panel titles", old="[data-viz-panel-key] h2", new="section[data-panel-type] h2, section[data-panel-type] h3", texts=True),
                F("table panel", old="[data-viz-panel-key] [role=table], [data-viz-panel-key] table", new="section[data-panel-type] table, section[data-panel-type] [role=grid]"),
                F("chart canvas / svg", old="[data-viz-panel-key] canvas, [data-viz-panel-key] svg", new="section[data-panel-type] canvas, section[data-panel-type] svg"),
                F("time range picker", old="[data-testid*='TimePicker']", new="[role=group] select, [role=group] [role=combobox]"),
                F("refresh control", old="button[aria-label*='efresh']", new="button:has-text('Refresh'), button:has-text('Obnovi')"),
                F("variables / templating", old="[data-testid*='template-variable']", new="[role=group] label"),
                F("export / share", old="button:has-text('Export'), button:has-text('Share')", new="a:has-text('tables'), a:has-text('tabuľky')"),
                F("breadcrumb / back", old="nav[aria-label*='readcrumb']", new="a[href$='/grafana']")]
GRAFANA_PAGES = [
    {"id": "dashboards", "old": "/grafana/dashboards", "new": "/projects/{project}/grafana",
     "features": [F("dashboard rows", old="a[href*='/d/']", new="a[href*='/grafana/dashboards/']"),
                  F("search input", old="input[placeholder*='earch']", new="input[type=search], input[placeholder]"),
                  F("folder rows", old="a[href*='/dashboards/f/']", new=None)]},
]  # one page per dashboard (Portal view and daiya's own tables) is added by resolve()

# --- Gitea vs shako ------------------------------------------------------------------------------
GITEA_REPO = [F("file tree", "#repo-files-table, table.files, .repo-file-list, .shako-files"),
              F("README render", "#readme, .markdown, .readme"),
              F("branch selector", ".branch-dropdown-button, .ui.dropdown.select-branch, a[href*='/branches']"),
              F("clone URL", "input#repo-clone-url, .clone-url, text=/\\.git\\b/"),
              F("commits link", "a[href*='/commits']"),
              F("pull requests tab", "a[href*='/pulls']"),
              F("settings tab", "a[href$='/settings']")]
R = "/projects/{project}/repositories/{prepo}"
GITEA_PAGES = [
    {"id": "repo-list", "old": "/git/explore/repos", "new": "/projects/{project}/repositories",
     "features": [F("repo rows", old=".flex-item-title a", new="a[href*='/repositories/']"),
                  F("search / filter input", old="input[name=q]", new="input[type=search], input[placeholder]"),
                  F("sort control", old=".ui.dropdown:has-text('Sort'), a[href*='sort=']", new="select, [role=combobox]")]},
    {"id": "repo-home", "old": "/git/{repo}", "new": R, "features": [
        F("file tree", old="#repo-files-table", new="table, [role=table], [role=grid]"),
        F("README render", old="#readme", new="article, .markdown, [data-readme]"),
        F("branch selector", old=".branch-dropdown-button", new="select, [role=combobox]"),
        F("clone URL", old="#repo-clone-url, .clone-panel", new="text=/\\.git\\b/"),
        F("commits tab", old="a[href*='/commits']", new="[role=tab]:has-text('Commit')"),
        F("pull requests tab", old="a[href*='/pulls']", new="[role=tab]:has-text('ull')"),
        F("settings tab", old="a[href$='/settings']", new="[role=tab]:has-text('Settings'), [role=tab]:has-text('Nastaven')")], "settle": 4000},
    {"id": "file", "old": "/git/{repo}/src/branch/{branch}/{file}", "new": R + "?tab=files&file={file}",
     "features": [F("file content", old=".file-view", new="pre, code"), F("raw link", old="a[href*='/raw/']", new="a[href*='raw'], a[download]"),
                  F("line numbers", old=".lines-num", new="[data-line-number], td:first-child")], "settle": 4000},
    {"id": "commits", "old": "/git/{repo}/commits/branch/{branch}", "new": R + "?tab=commits",
     "features": [F("commit rows", old="#commits-table tbody tr, .commit-list a.sha", new="a[href*='commit=']"),
                  F("author", old=".author-wrapper, .commit-author", new="text=/@|[Aa]uthor|[Aa]utor/")], "settle": 4000},
    {"id": "commit-diff", "old": "/git/{repo}/commit/{sha}", "new": R + "?tab=commits",
     "then": [{"click": "a[href*='commit=']"}],
     "features": [F("diff file headers", old=".diff-file-header", new="h3, h4, summary"),
                  F("added/removed lines", old=".add-code, .del-code", new="[class*=success], [class*=danger], ins, del")], "settle": 4000},
    {"id": "branches", "old": "/git/{repo}/branches", "new": R + "?tab=branches", "features": [F("branch rows", old="#branch-table tr, .branch-name", new="table tr, li")], "settle": 4000},
    {"id": "pulls", "old": "/git/{repo}/pulls?state=all", "new": R + "?tab=pulls",
     "features": [F("pull rows", old="a[href*='/pulls/']", new="a[href*='pull=']"), F("open/closed filter", old="a[href*='state=closed']", new="select, [role=radiogroup], [role=tablist] [role=tab]")], "settle": 4000},
    {"id": "pull", "old": "/git/{repo}/pulls/{pull}", "new": R + "?tab=pulls&pull={pull}",
     "features": [F("conversation / description", old=".timeline, .comment", new="article, p"),
                  F("files changed", old="a[href$='/files']", new="text=/[Ff]iles|[Ss]úbory/"),
                  F("merge button", old="button:has-text('Merge'), .merge-section", new="button:has-text('Merge'), button:has-text('Zlúč')"),
                  F("state label", old=".issue-state-label", new="[class*=badge], [class*=chip]")], "settle": 4000},
    {"id": "settings", "old": "/git/{repo}/settings", "new": R + "?tab=settings", "features": [F("settings form or refusal", "form, [role=alert], h1")], "settle": 4000},
]

# --- Keycloak vs kaisatsu ------------------------------------------------------------------------
KC_LOGIN = [F("username field", "input[name=username]"), F("password field", "input[name=password]"),
            F("submit button", "input[type=submit], button[type=submit]"), F("realm/brand header", "#kc-header, header, h1"),
            F("forgot password link", "a[href*='reset-credentials']"), F("locale switch", "#kc-locale, [aria-label*='anguage'], select[name=kc_locale]")]
KC_PAGES = [
    {"id": "login", "anon": True, "old": f"https://portal.{DOMAIN}/api/v1/auth/login", "new": "/api/v1/auth/login", "features": KC_LOGIN},
    {"id": "login-error", "anon": True, "old": None, "why": "not tried on dev: a failed sign-in writes a login event there",
     "new": "/api/v1/auth/login",
     "then": [{"bad_login": True}], "features": KC_LOGIN + [F("error message", "#input-error, .kc-feedback-text, [role=alert], .alert-error, .error")]},
    {"id": "account", "old": "/realms/dev/account/", "new": None, "why": "kaisatsu serves no account console (/realms/{realm}/account)",
     "features": [F("personal info form", "input[name=email], #email")]},
    {"id": "account-security", "old": "/realms/dev/account/account-security/signing-in", "new": None,
     "why": "kaisatsu serves no account console: no self-service password change or OTP setup page",
     "features": [F("password row", "text=/[Pp]assword|[Hh]eslo/"), F("2FA / OTP row", "text=/OTP|[Aa]uthenticator|[Tt]wo-factor/")]},
    {"id": "account-sessions", "old": "/realms/dev/account/account-security/device-activity", "new": None,
     "why": "kaisatsu serves no account console: no device activity / sign out other sessions page",
     "features": [F("session rows", "text=/[Ss]ession|[Dd]evice/")]},
    {"id": "people", "old": "/admin/dev/console/#/dev/users", "new": "/organization/people",
     "features": [F("people table", "table, [role=grid]"), F("search", "input[type=search], input[placeholder]")]},
]

# --- Portal pipeline pages (Bento runner vs ekiben) ----------------------------------------------
PIPE = [F("runs table", "table, [role=table], [role=grid]"), F("status chip", "[class*=chip], [class*=badge], [data-status]"),
        F("run log", "text=/Log of the run|Záznam behu/"), F("Run now button", "button:has-text('Run now'), button:has-text('Spustiť')"),
        F("Pause / Edit", "button:has-text('Pause'), a:has-text('Edit'), button:has-text('Edit')"),
        F("source vs space", "text=/Source vs space|Zdroj verzus priestor/"), F("error / refusal banner", "[role=alert]")]
WORKBENCH = [F("sample of the source", "text=/[Ss]ample|[Vv]zork/"), F("mapping editor", ".cm-editor, textarea, [role=textbox]"),
             F("try / test button", "button:has-text('Try'), button:has-text('Test'), button:has-text('Skúsi')"),
             F("output preview table", "table, [role=grid]"), F("error / refusal banner", "[role=alert]")]
PIPE_PAGES = [
    {"id": "pipelines", "old": "/projects/{project}/pipelines", "new": "/projects/{project}/pipelines", "features": PIPE, "settle": 4000},
    {"id": "pipeline", "old": "/projects/{project}/pipelines/{pipeline}", "new": "/projects/{project}/pipelines/{pipeline}", "features": PIPE, "settle": 6000},
    {"id": "pipeline-workbench", "old": "/projects/{project}/pipelines/{pipeline}/edit", "new": "/projects/{project}/pipelines/{pipeline}/edit",
     "features": WORKBENCH, "settle": 10000},
]

# --- Portal --------------------------------------------------------------------------------------
GRID = F("data grid", ".jc-grid, [role=grid], table")
PORTAL_COMMON = [F("h1", "h1"), F("sidebar nav", "nav a[href*='/projects/']"), F("error / refusal banner", "[role=alert]"),
                 F("empty state", "text=/[Nn]othing|[Nn]o [a-z]+ yet|[Ee]mpty|Žiadn/")]
PORTAL_PAGES = [{"id": i, "old": p, "new": p, "features": PORTAL_COMMON + extra, "settle": s} for i, p, extra, s in [
    ("catalogue", "/catalogue", [GRID, F("search", "input[type=search], input[placeholder]")], 4000),
    ("endpoints-all", "/endpoints", [GRID], 3000),
    ("organization", "/organization", [GRID], 3000),
    ("project-home", "/projects/{project}/home", [F("cards", "[class*=card], section")], 4000),
    ("spaces", "/projects/{project}/spaces", [GRID], 3000),
    ("models", "/projects/{project}/models", [GRID], 3000),
    ("endpoints", "/projects/{project}/endpoints", [GRID], 3000),
    ("endpoint", "/projects/{project}/endpoints/{endpoint}", [GRID, F("map", ".maplibregl-map, canvas")], 6000),
    ("explore", "/projects/{project}/explore", [GRID, F("map", ".maplibregl-map, canvas"), F("type picker", "select, [role=combobox]")], 8000),
    ("datasources", "/projects/{project}/datasources", [GRID], 3000),
    ("subscriptions", "/projects/{project}/subscriptions", [GRID], 3000),
    ("mqttbrokers", "/projects/{project}/mqttbrokers", [GRID], 3000),
    ("grafana", "/projects/{project}/grafana", [F("dashboard links / chart", "a[href*='/d/'], iframe, canvas, svg")], 6000),
    ("apps", "/projects/{project}/apps", [GRID], 3000),
    ("syncsources", "/projects/{project}/syncsources", [GRID], 3000),
    ("assistant", "/projects/{project}/assistant", [], 3000),
    ("workspaces", "/projects/{project}/workspaces", [GRID], 3000),
    ("approvals", "/projects/{project}/approvals", [GRID], 3000),
    ("activity", "/projects/{project}/activity", [GRID], 3000),
    ("health", "/projects/{project}/health", [], 4000),
    ("policies", "/projects/{project}/policies", [GRID], 3000),
    ("members", "/projects/{project}/settings/members", [GRID, F("member rows", "main table tbody tr")], 4000),
    ("ckan", "/projects/{project}/ckan", [GRID], 3000),
    ("knowledge", "/projects/{project}/knowledge", [], 3000),
    ("mcp", "/projects/{project}/mcp", [], 3000),
    ("glossary", "/glossary", [], 2000),
]]

PAIRS = {
    "ckan": {"title": "CKAN (jc_theme) vs dataroom", "pages": CKAN_PAGES},
    "assistant": {"title": "AI assistant (production's assistant and the catalogue launcher) vs jc", "pages": ASSIST_PAGES},
    "grafana": {"title": "Grafana vs daiya", "pages": GRAFANA_PAGES},
    "gitea": {"title": "Gitea vs shako", "pages": GITEA_PAGES},
    "keycloak": {"title": "Keycloak vs kaisatsu", "pages": KC_PAGES},
    "pipelines": {"title": "Bento runner vs ekiben (Portal pipeline pages)", "pages": PIPE_PAGES},
    "portal": {"title": "Portal on dev vs Portal in jc", "pages": PORTAL_PAGES},
}


def _first(items, key):
    for i in items or []:
        if i.get(key):
            return i[key]
    return None


def add_dashboards(found: list) -> None:
    """One page per dashboard (and its daiya table page). The new side and compose take the old side's
    list from vars-old.json, so a dashboard daiya does not list to the person still gets its row."""
    known = {p["id"] for p in GRAFANA_PAGES}
    for d in found:
        pid = "dash-" + d["uid"]
        if pid in known:
            continue
        GRAFANA_PAGES.append({"id": pid, "old": f"/grafana/d/{d['uid']}", "new": "/projects/{project}/grafana/dashboards/" + d["uid"],
                              "features": GRAFANA_DASH, "settle": 8000, "title": d.get("title")})
        GRAFANA_PAGES.append({"id": pid + "-daiya-tables", "old": None, "why": "daiya's own server-side page has no Grafana twin",
                              "new": f"/dashboards/d/{d['uid']}", "features": [F("tables", "table")], "settle": 3000})
        known.add(pid)


def resolve(pair: str, side: str, base: str, page) -> dict:
    """Names a page path needs, read on that side with GETs only."""
    b = base_of(pair, side, base)
    v = {"project": PROJECT, "dataset": DATASET, "org": PROJECT}
    if pair == "ckan":
        pre = "/opendata" if side == "new" else ""
        d = _get(page, f"{b}{pre}/api/3/action/package_show?id={DATASET}")
        res = (d or {}).get("result", {}).get("resources", [])
        csv = next((r for r in res if r.get("format") == "CSV"), res[0] if res else None)
        if csv:
            v[f"resource_{side}"] = csv["id"]
    elif pair == "grafana":
        pre = "/grafana" if side == "old" else "/dashboards"
        if side == "old":
            _visit(page, f"{b}{pre}/")  # the session cookie of the dashboards host comes with the first page
        found = _get(page, f"{b}{pre}/api/search?type=dash-db&limit=200") or []
        v["dashboards"] = [{"uid": d["uid"], "title": d.get("title")} for d in found if d.get("uid")]
        add_dashboards(v["dashboards"])
    elif pair == "gitea" and side == "new":
        pass  # the Portal's repository pages take the names the old side resolved (shako's API wants a token)
    elif pair == "gitea":
        pre = "/git"
        auth = None
        if OLD_GITEA_LOCAL:
            pre = ""
            user, password = Path(os.environ["PARITY_OLD_GITEA_PERSON"]).read_text().split()[:2]
            _visit(page, f"{b}/user/login")
            page.fill("#user_name", user)
            page.fill("#password", password)
            page.locator("#password").press("Enter")
            page.wait_for_timeout(2000)
            # Gitea's API takes no session cookie: the stand-in's own person by basic auth.
            import base64
            auth = {"Authorization": "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()}
        else:  # Gitea signs in through Keycloak only on this click; the session is already there
            _visit(page, f"{b}/git/user/oauth2/keycloak")
        found = _get(page, f"{b}{pre}/api/v1/repos/search?limit=50&q={PROJECT}", auth) or {}
        repos = found.get("data", []) if isinstance(found, dict) else found
        repo = next((r for r in repos if r.get("full_name", "").endswith(f"/{PROJECT}")), repos[0] if repos else None)
        if repo:
            full = repo["full_name"]
            v["repo"] = full
            v["branch"] = repo.get("default_branch", "main")
            commits = _get(page, f"{b}{pre}/api/v1/repos/{full}/commits?limit=1", auth) or []
            if commits:
                v["sha"] = commits[0]["sha"]
            tree = _get(page, f"{b}{pre}/api/v1/repos/{full}/contents?ref={v['branch']}", auth) or []
            f = next((t["path"] for t in tree if t.get("type") == "file"), None)
            if f:
                v["file"] = quote(f)
            pulls = _get(page, f"{b}{pre}/api/v1/repos/{full}/pulls?state=all&limit=1", auth) or []
            if pulls:
                v["pull"] = pulls[0]["number"]
            v["prepo"] = full.split("/")[-1]
    elif pair in ("pipelines", "portal"):
        pipes = _get(page, f"{b}/api/v1/projects/{PROJECT}/pipelines") or {}
        items = pipes.get("items", pipes if isinstance(pipes, list) else [])
        name = _first([i.get("metadata", {}) for i in items], "name")
        if name:
            v["pipeline"] = name
        eps = _get(page, f"{b}/api/v1/projects/{PROJECT}/endpoints") or {}
        items = eps.get("items", eps if isinstance(eps, list) else [])
        name = _first([i.get("metadata", {}) for i in items], "name")
        if name:
            v["endpoint"] = name
    return v


def act(page, action: dict, side: str):
    if "click_tab" in action:
        page.get_by_role("tab", name=__import__("re").compile(action["click_tab"].strip("/"))).first.click(timeout=5000)
    elif "click" in action:
        page.locator(action["click"]).first.click(timeout=10000)
        page.wait_for_load_state("networkidle")
    elif action.get("bad_login"):
        page.fill("input[name=username]", "parity-nobody")
        page.fill("input[name=password]", "wrong-password-1")
        page.locator("input[name=password]").press("Enter")
        page.wait_for_timeout(2500)
    elif "goto" in action:
        page.goto(action["goto"])
