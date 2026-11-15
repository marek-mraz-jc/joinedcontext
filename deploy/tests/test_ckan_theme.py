"""T-2744 (DS-01, UI-82): the catalogue's dataset pages are written for people.

`ckanext_jc_theme/plugin.py` turns a dataset's CKAN fields and the extras `jcctl` publishes
(crates/jcctl/src/publish/ckan.rs) into labelled rows, the live API box and resources grouped
by format. The helpers are plain functions, so they are tested here against a stand-in for
the two CKAN modules they import; the templates that call them were walked on a CKAN 2.11.6
(the image's base) with axe clean on every page.
"""

import importlib.util
import datetime
import html as html_lib
import json
import os
import re
import shutil
import subprocess
import sys
import types
from pathlib import Path
from urllib.parse import urlsplit
from xml.etree import ElementTree

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
THEME = PROJECT_ROOT / "charts/ckan/files/jc_theme"
# The Portal's four languages and Hungarian for southern BBSK (T-5115).
LANGUAGES = {"en", "sk", "cs", "de", "hu"}


@pytest.fixture
def theme(tmp_path, monkeypatch):
    """The plugin module, loaded with a branding block for `dev.example` and the page in `lang`."""
    branding = tmp_path / "branding.json"
    branding.write_text(json.dumps({"domain": "dev.example", "instanceName": "joinedcontext"}))
    monkeypatch.setenv("JC_BRANDING_FILE", str(branding))

    page = types.SimpleNamespace(lang="en")
    toolkit = types.ModuleType("ckan.plugins.toolkit")
    toolkit.h = types.SimpleNamespace(lang=lambda: page.lang)
    # CKAN's actions, answered from `page.actions` by name: a callable or an exception to raise.
    page.actions = {}

    def get_action(name):
        def action(context, data_dict):
            answer = page.actions[name]
            if isinstance(answer, Exception):
                raise answer
            return answer(data_dict)
        return action

    for error in ("ObjectNotFound", "NotAuthorized", "ValidationError", "Invalid"):
        setattr(toolkit, error, type(error, (Exception,), {}))
    toolkit.get_action = get_action
    toolkit.chained_helper = lambda helper: helper
    toolkit.get_validator = lambda name: (lambda value: value)
    plugins = types.ModuleType("ckan.plugins")
    plugins.toolkit = toolkit
    plugins.SingletonPlugin = object
    plugins.implements = lambda *args, **kwargs: None
    page.loaded = set()
    plugins.plugin_loaded = lambda name: name in page.loaded
    plugins.IConfigurer = plugins.ITemplateHelpers = plugins.IFacets = plugins.IPackageController = object
    plugins.IBlueprint = plugins.IResourceView = plugins.IFeed = object
    ckan = types.ModuleType("ckan")
    ckan.__file__ = str(tmp_path / "ckan/__init__.py")
    ckan.plugins = plugins
    for name, module in (("ckan", ckan), ("ckan.plugins", plugins), ("ckan.plugins.toolkit", toolkit)):
        monkeypatch.setitem(sys.modules, name, module)

    # No __pycache__ beside the theme: the chart ships every file of that directory.
    monkeypatch.setattr(sys, "dont_write_bytecode", True)
    spec = importlib.util.spec_from_file_location("jc_theme_plugin", THEME / "plugin.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.page = page
    return module


ENDPOINT = "https://dev.example/api/endpoint/hkiair/"

PUBLISHED = {
    "metadata_modified": "2026-09-25T08:00:00.000000",
    "organization": {"name": "helsinki", "title": "City of Helsinki"},
    "license_id": "cc-by",
    "license_title": "Creative Commons Attribution",
    "license_url": "http://www.opendefinition.org/licenses/cc-by",
    "groups": [{"display_name": "Environment"}],
    "tags": [{"display_name": "air quality"}, {"display_name": "pollution"}],
    "extras": [
        {"key": "frequency", "value": "http://publications.europa.eu/resource/authority/frequency/DAILY"},
        {"key": "publisher_name", "value": "Environment Services"},
        {"key": "publisher_uri", "value": "https://www.hel.fi/"},
        {"key": "contact_email", "value": "data@hel.fi"},
        {"key": "temporal_start", "value": "2024-01-01"},
        {"key": "endpoint", "value": ENDPOINT},
        {"key": "generated_by", "value": "jcctl/ckan-publisher"},
        {"key": "source_note", "value": "Measured by HSY"},
    ],
    "resources": [
        {"format": "CSV"}, {"format": "NGSI-LD"}, {"format": "csv"}, {"format": ""},
    ],
}


def test_the_about_list_names_publisher_licence_and_freshness_in_plain_words(theme):
    rows = {label: (text, link) for label, text, link in theme.jc_about(PUBLISHED)}
    assert rows["Publisher"] == ("Environment Services", "https://www.hel.fi/")
    assert rows["Licence"] == ("Creative Commons Attribution", "http://www.opendefinition.org/licenses/cc-by")
    assert rows["Topic"] == ("Environment", None)
    assert rows["Keywords"] == ("air quality, pollution", None)
    assert rows["Updated"] == ("Every day", None)
    assert rows["Last update"] == ("2026-09-25", None)
    assert rows["Next update expected"] == ("2026-09-26", None)
    assert rows["Contact"] == ("data@hel.fi", "mailto:data@hel.fi")
    assert rows["Time covered"] == ("2024-01-01", None)


def test_an_hourly_dataset_promises_an_hour_and_an_irregular_one_no_date(theme):
    hourly = dict(PUBLISHED, extras=[{"key": "frequency", "value": "HOURLY"}])
    rows = {label: text for label, text, _ in theme.jc_about(hourly)}
    # The hour follows the page's own date, never CKAN's "%B %d, %Y" order (T-4333).
    assert rows["Next update expected"] == "2026-09-25, 09:00 (UTC)"
    irregular = dict(PUBLISHED, extras=[{"key": "frequency", "value": ".../frequency/IRREG"}])
    rows = {label: text for label, text, _ in theme.jc_about(irregular)}
    assert rows["Updated"] == "Irregularly" and "Next update expected" not in rows
    label, next_update = theme.jc_frequency(dict(PUBLISHED, metadata_modified="not a date"))
    assert (label, next_update) == ("Every day", None)


def test_created_is_the_true_origin_and_updated_the_last_real_change(theme):
    # T-4644: the publisher's (or our restored) `issued` before CKAN's re-creation date, the
    # source's `modified` before CKAN's own; a value that is no date falls back, never shows.
    restored = dict(PUBLISHED, metadata_created="2026-10-16T14:14:13.049715",
                    extras=PUBLISHED["extras"] + [{"key": "issued", "value": "2026-09-07T09:00:00Z"}])
    assert theme.jc_created(restored).isoformat() == "2026-09-07T09:00:00+00:00"
    assert theme.jc_updated(restored).isoformat() == "2026-09-25T08:00:00+00:00"
    rows = {label: text for label, text, _ in theme.jc_about(restored)}
    assert rows["Created"] == "2026-09-07" and rows["Last update"] == "2026-09-25"
    assert "issued" not in dict(theme.jc_more(restored))
    harvested = dict(restored, extras=[{"key": "issued", "value": "2019-04-02"}, {"key": "modified", "value": "2023-11-30T08:26:58Z"}])
    assert theme.jc_created(harvested).date().isoformat() == "2019-04-02"
    assert theme.jc_updated(harvested).isoformat() == "2023-11-30T08:26:58+00:00"
    assert theme.jc_frequency(dict(harvested, extras=harvested["extras"] + [{"key": "frequency", "value": "DAILY"}]))[1].date().isoformat() == "2023-12-01"
    broken = dict(restored, extras=[{"key": "issued", "value": "unknown"}])
    assert theme.jc_created(broken).isoformat() == "2026-10-16T14:14:13.049715+00:00"
    assert theme.jc_created({}) is None and theme.jc_updated({}) is None
    for template in ("templates__home__index.html", "templates__snippets__package_item.html"):
        page = (THEME / template).read_text()
        assert "h.jc_updated(" in page and "metadata_modified" not in page, template


def test_a_bare_dataset_shows_only_what_it_has_and_the_publisher_falls_back_to_the_organization(theme):
    assert theme.jc_about({}) == []
    bare = {"organization": {"name": "helsinki", "title": "City of Helsinki"}}
    assert theme.jc_about(bare) == [("Publisher", "City of Helsinki", None)]
    assert theme.jc_live({}) is None and theme.jc_resource_sections({}) == [] and theme.jc_more({}) == []
    assert theme.jc_previews({}) == [] and theme.jc_formats({}) == []


def test_a_link_that_is_not_http_is_shown_as_text(theme):
    hostile = dict(PUBLISHED, license_url="javascript:alert(1)",
                   extras=[{"key": "publisher_name", "value": "X"}, {"key": "publisher_uri", "value": "javascript:alert(1)"}])
    for _, _, link in theme.jc_about(hostile):
        assert link is None or link.startswith(("https://", "http://", "mailto:")), link


def test_the_live_box_shows_only_an_https_address_of_the_platform_and_a_query_that_answers(theme):
    live = theme.jc_live(PUBLISHED)
    assert live["url"] == ENDPOINT
    assert live["example"].endswith(f"'{ENDPOINT}ngsi-ld/v1/types'")
    for endpoint in ("http://dev.example/api/endpoint/x/", "https://context-gateway.jc.svc:8080/x/",
                     "https://dev.example.evil/x/", "https://evildev.example/x/"):
        foreign = dict(PUBLISHED, extras=[{"key": "endpoint", "value": endpoint}])
        assert theme.jc_live(foreign) is None, endpoint
    sub = dict(PUBLISHED, extras=[{"key": "endpoint", "value": "https://api.dev.example/e/x"}])
    assert theme.jc_live(sub)["url"] == "https://api.dev.example/e/x/"


def test_internal_extras_never_reach_more_details(theme):
    assert theme.jc_more(PUBLISHED) == [("source_note", "Measured by HSY")]
    # T-3025: the DCAT metadata jcctl writes for harvesters is not words for a reader: the EU
    # licence URI beside the licence the About list already links, and the themes as a JSON list.
    dcat = dict(PUBLISHED, extras=PUBLISHED["extras"] + [
        {"key": "license_url", "value": "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0"},
        {"key": "theme", "value": '["http://publications.europa.eu/resource/authority/data-theme/TRAN"]'},
        {"key": "spatial_uri", "value": "http://data.europa.eu/nuts/code/CZ010"},
    ])
    assert theme.jc_more(dcat) == [("source_note", "Measured by HSY")]
    rows = {label: text for label, text, _ in theme.jc_about(dcat)}
    assert rows["Area covered"] == "Prague (NUTS CZ010)"  # linked to its definition (T-4333)


def test_the_attribution_and_the_original_data_are_about_rows_and_never_more_details(theme):
    """T-4417: the credit the licence asks for and the data it came from, from jcctl's extras;
    an address that is not http stays out, and the pipeline reads in words under More details."""
    credited = dict(PUBLISHED, extras=PUBLISHED["extras"] + [
        {"key": "rights", "value": "Štatistický úrad SR, CC BY-SA 4.0"},
        {"key": "source", "value": '["https://data.statistics.sk/api/v2/dataset/as1001rs", "javascript:alert(1)"]'},
        {"key": "was_generated_by", "value": "pipeline susr-as"},
    ])
    rows = [(label, text, link) for label, text, link in theme.jc_about(credited)]
    assert ("Attribution", "Štatistický úrad SR, CC BY-SA 4.0", None) in rows
    sources = [(text, link) for label, text, link in rows if label == "Original data"]
    assert sources == [("https://data.statistics.sk/api/v2/dataset/as1001rs",) * 2]
    assert ("was_generated_by", "pipeline susr-as") in theme.jc_more(credited)
    assert theme.jc_extra("was_generated_by", "pipeline susr-as")[0] == "Produced by"
    assert not any(k in ("rights", "source") for k, _ in theme.jc_more(credited))
    # T-4436: jcctl names each source with its title; the row shows the title, links the address.
    titled = dict(PUBLISHED, extras=[{"key": "source", "value": json.dumps([
        {"url": "https://data.statistics.sk/api/v2/dataset/as1001rs", "title": "Obyvateľstvo", "description": "x"},
        {"url": "javascript:alert(1)", "title": "Click"},
        {"url": "https://example.org/untitled"}])}])
    assert [(text, link) for label, text, link in theme.jc_about(titled) if label == "Original data"] == [
        ("Obyvateľstvo", "https://data.statistics.sk/api/v2/dataset/as1001rs"),
        ("https://example.org/untitled", "https://example.org/untitled")]
    for broken in ("[not json", "{}", "ftp://x"):
        bad = dict(PUBLISHED, extras=[{"key": "source", "value": broken}])
        assert not [r for r in theme.jc_about(bad) if r[0] == "Original data"], broken


SCHEMA = "https://dev.example/api/endpoint/praha/schema/v1/"

# The resources `jcctl` publishes for an Endpoint, in its order: the DataStore table last.
PUBLISHED_RESOURCES = [
    {"id": "r-ngsi", "name": "NGSI-LD API", "format": "NGSI-LD", "url": ENDPOINT + "ngsi-ld/v1/"},
    {"id": "r-csv", "name": "CSV", "format": "CSV", "url": ENDPOINT + "file.csv"},
    {"id": "r-mcp", "name": "Model Context Protocol", "format": "MCP", "url": ENDPOINT + "mcp"},
    {"id": "r-index", "name": "Schema artifacts", "format": "JSON", "url": ENDPOINT + "schema/index.json"},
    {"id": "r-shacl", "name": "model.shacl.ttl", "format": "SHACL", "url": SCHEMA + "model.shacl.ttl"},
    {"id": "r-ctx", "name": "context.jsonld", "format": "JSON-LD", "url": SCHEMA + "context.jsonld?v=1"},
    {"id": "r-odd", "name": "Unlabelled", "format": "", "url": ENDPOINT + "other"},
    {"id": "r-table", "name": "DataStore", "format": "CSV", "url": "https://data.dev.example/datastore/dump/r-table",
     "datastore_active": True},
]


def test_the_datastore_table_leads_the_files_and_the_model_is_a_section_of_its_own(theme):
    """T-3009: the DataStore resource was the last of fourteen links; it leads the data now."""
    sections = theme.jc_resource_sections({"resources": PUBLISHED_RESOURCES})
    ids = {s["key"]: [i["resource"]["id"] for i in s["items"]] for s in sections}
    assert [s["key"] for s in sections] == ["data", "api", "schema"]
    assert ids["data"] == ["r-table", "r-csv", "r-odd"]
    assert ids["api"] == ["r-ngsi", "r-mcp"]
    # Everything under the Endpoint's /schema/ path is the model, whatever format it names.
    assert sorted(ids["schema"]) == ["r-ctx", "r-index", "r-shacl"]
    data = sections[0]["items"]
    assert data[0]["what"] == "The table above, as one file"
    assert data[1]["what"] == "A table for a spreadsheet"
    assert data[-1]["format"] == "—" and data[-1]["what"] == ""
    assert sections[2]["title"] == "Data model" and sections[1]["lead"]


def test_a_dataset_with_only_files_has_one_section_and_a_search_result_names_no_model_format(theme):
    files = [r for r in PUBLISHED_RESOURCES if r["id"] in ("r-csv",)]
    assert [s["key"] for s in theme.jc_resource_sections({"resources": files})] == ["data"]
    assert theme.jc_formats({"resources": PUBLISHED_RESOURCES}) == ["CSV", "NGSI-LD", "MCP"]


# Since T-3012 the mirror writes one DataStore table per entity type, named by the type and
# with no format of its own.
TYPE_TABLES = [
    {"id": "t-bikes", "name": "BikeHireDockingStation", "format": "", "url": "https://data.dev.example/datastore/dump/t-bikes",
     "datastore_active": True},
    {"id": "t-air", "name": "AirQualityObserved", "format": "", "url": "https://data.dev.example/datastore/dump/t-air",
     "datastore_active": True},
]


def test_the_page_frames_every_type_table_with_its_row_count(theme):
    rows = {"t-air": 300, "t-bikes": 24808, "r-table": 5}
    theme.page.actions = {
        "resource_view_list": lambda d: [{"id": "v-image", "view_type": "image_view"},
                                         {"id": "v-" + d["id"], "view_type": "datatables_view"}],
        "datastore_search": lambda d: {"total": rows[d["resource_id"]]} if d["limit"] == 0 else {},
    }
    previews = theme.jc_previews({"resources": PUBLISHED_RESOURCES[:3] + TYPE_TABLES})
    assert [p["resource"]["id"] for p in previews] == ["t-air", "t-bikes"]
    assert [p["view"]["id"] for p in previews] == ["v-t-air", "v-t-bikes"]
    assert theme.jc_total(previews) == 25108 and theme.jc_number(25108) == "25\u202f108"
    # A table the mirror wrote without a format downloads as CSV and says what it is.
    data = theme.jc_resource_sections({"resources": TYPE_TABLES})[0]["items"]
    assert [(i["format"], i["what"]) for i in data] == [("CSV", "The table above, as one file")] * 2


def test_a_table_without_a_view_or_a_count_still_shows_and_says_so(theme):
    theme.page.actions = {
        "resource_view_list": theme.toolkit.NotAuthorized(),
        "datastore_search": theme.toolkit.ObjectNotFound(),
    }
    previews = theme.jc_previews({"resources": PUBLISHED_RESOURCES})
    assert previews == [{"resource": PUBLISHED_RESOURCES[-1], "view": None, "total": None}]
    assert theme.jc_total(previews) is None and theme.jc_total([]) is None
    assert theme.jc_number(None) == "" and theme.jc_number("x") == ""
    read = (THEME / "templates__package__read.html").read_text()
    assert "h.jc_t('no_view')" in read and "total is not none" in read and "preview.total is not none" in read


def test_only_the_first_table_is_open_so_the_others_load_nothing_until_asked():
    read = (THEME / "templates__package__read.html").read_text()
    assert '<details class="jc-table"{% if loop.first %} open{% endif %}>' in read
    assert 'loading="lazy"' in read


def test_the_table_comes_before_the_about_list_on_the_dataset_page():
    read = (THEME / "templates__package__read.html").read_text()
    assert read.index("h.jc_previews(pkg)") < read.index("h.jc_about(pkg)") < read.index("h.jc_live(pkg)")
    # The framed table carries a title a screen reader announces.
    assert re.search(r"<iframe[^>]*title=\"\{\{ h.jc_t\('table_title'\) \}\}: \{\{ name \}\}", read)


def test_the_mcp_panel_names_the_installation_s_hub_and_nothing_else(theme):
    """T-4114, EP-113: one address for the whole catalogue, a ready line per client."""
    from base64 import b64decode
    from urllib.parse import parse_qs, unquote, urlsplit

    mcp = theme.jc_mcp_install()
    hub = "https://api.dev.example/api/mcp"
    assert mcp["url"] == hub
    # One argument per line and the address alone on its JSON line, so no line is wider than the
    # address a narrow card has to show anyway (T-4284).
    assert mcp["claude"] == f"claude mcp add --transport http \\\n  joinedcontext \\\n  {hub}"
    assert json.loads(mcp["json"]) == {"mcpServers": {"joinedcontext": {"type": "http", "url": hub}}}
    assert f'  "{hub}"' in mcp["json"].splitlines()
    # The buttons copy the command on one line, so it runs as pasted in cmd and PowerShell too,
    # and the JSON in its usual layout (T-4762, owner on Windows).
    assert mcp["claude_copy"] == f"claude mcp add --transport http joinedcontext {hub}"
    assert "\n" not in mcp["claude_copy"] and "\\" not in mcp["claude_copy"]
    assert json.loads(mcp["json_copy"]) == json.loads(mcp["json"])
    assert f'      "url": "{hub}"' in mcp["json_copy"].splitlines()
    # The address breaks only before a single slash, never inside a word.
    assert mcp["url_parts"] == ["https://api.dev.example", "/api", "/mcp"]
    assert mcp["vscode"].startswith("vscode:mcp/install?")
    assert json.loads(unquote(mcp["vscode"].split("?", 1)[1])) == {"name": "joinedcontext", "type": "http", "url": hub}
    cursor = urlsplit(mcp["cursor"])
    assert (cursor.scheme, cursor.netloc, cursor.path) == ("cursor", "anysphere.cursor-deeplink", "/mcp/install")
    query = parse_qs(cursor.query)
    assert query["name"] == ["joinedcontext"]
    assert json.loads(b64decode(query["config"][0])) == {"url": hub}
    # No dataset, no token: every line is the same for every page (EP-110).
    for line in (v for v in mcp.values() if isinstance(v, str)):
        assert "endpoint/" not in unquote(line) and "token" not in line.lower() and "bearer" not in line.lower()


@pytest.mark.parametrize("server, path", [
    ("d/bbsk-demo", "/api/mcp/d/bbsk-demo"),
    # Anything but `d/{publicId}` leaves the hub: no other path, no climbing, no query.
    ("d/../admin", "/api/mcp"), ("x/bbsk-demo", "/api/mcp"), ("d/Bbsk", "/api/mcp"),
    ("d/bbsk?x=1", "/api/mcp"), ("d/", "/api/mcp"), (7, "/api/mcp"), (None, "/api/mcp"),
])
def test_a_demo_offers_its_assistant_s_own_server_and_nothing_else(theme, server, path):
    """T-4687: branding `mcpServer` names one assistant's server (T-4691) for every client line."""
    theme.BRANDING["mcpServer"] = server
    mcp = theme.jc_mcp_install()
    url = "https://api.dev.example" + path
    assert mcp["url"] == url
    assert mcp["claude"].endswith(url) and json.loads(mcp["json"])["mcpServers"]["joinedcontext"]["url"] == url


@pytest.mark.parametrize("domain", [None, "", "dev.example\" onmouseover=\"x", "dev.example/evil", "localhost", 7])
def test_the_mcp_panel_is_left_out_without_a_plain_domain(theme, domain):
    theme.BRANDING["domain"] = domain
    assert theme.jc_mcp_install() is None


def test_the_mcp_panel_is_on_the_assistants_connector_and_every_dataset_page_with_its_copy_script():
    panel = (THEME / "templates__snippets__jc_mcp_install.html").read_text()
    for page in ("templates__jc__assistants.html", "templates__package__read.html", "templates__jc__page.html"):
        text = (THEME / page).read_text()
        assert re.search(r"\{% snippet ['\"]snippets/jc_mcp_install.html['\"]", text), page
    # T-4762: /connector showed three dead buttons, its page never loaded the script. base.html
    # loads it once for every page; a second copy would copy and announce twice.
    loads = [f.name for f in THEME.glob("templates__*.html") if "/jc-copy.js" in f.read_text()]
    assert loads == ["templates__base.html"]
    # Every copy button says what to do when the browser refuses the clipboard.
    for f in THEME.glob("templates__*.html"):
        for button in re.findall(r"<button[^>]*data-jc-copy=[^>]*>", f.read_text()):
            assert "data-jc-select=\"{{ h.jc_t('copy_select') }}\"" in button, f.name
    assert "{{ copy('jc-mcp-claude', h.jc_t('mcp_claude'), mcp.claude_copy) }}" in panel
    assert "{{ copy('jc-mcp-json', h.jc_t('mcp_json'), mcp.json_copy) }}" in panel
    # The home page keeps the corner button only (T-4585).
    assert "jc_mcp_install.html" not in (THEME / "templates__home__index.html").read_text()
    # One status line per page: the live box's when the dataset page has one.
    assert '{% if not live %}<p class="visually-hidden" role="status"' in panel
    assert panel.count("{{ copy('") == 3  # the address, Claude Code and JSON (T-4628; no Gemini CLI since T-4735)


def _copy_click(clipboard, exec_copy, text=None, field_value=None):
    """Runs jc-copy.js on one button in node with a DOM of the few calls it makes, and returns
    what reached the clipboard, the selection and the words the page said."""
    script = (THEME / "public__jc-copy.js").read_text()
    code = """
const state = { clipboard: null, selected: null, status: "", label: "Kopírovať", execs: [] };
const field = %s === null
  ? { textContent: "claude mcp add --transport http \\\\\\n  joinedcontext \\\\\\n  https://api.x/api/mcp" }
  : { value: %s, select() { state.selected = this.value; } };
const label = { textContent: "Kopírovať", attrs: {}, getAttribute(n) { return this.attrs[n] ?? null; },
  setAttribute(n, v) { this.attrs[n] = v; } };
const status = { set textContent(v) { state.status = v; } };
const button = { attrs: { "data-jc-copy": "f", "data-jc-copied": "Skopírované", "data-jc-select": "Označené: stlačte Ctrl+C" },
  getAttribute(n) { return this.attrs[n] ?? null; }, querySelector() { return label; } };
if (%s !== null) button.attrs["data-jc-text"] = %s;
let listener;
let area;
globalThis.window = { getSelection: () => ({ removeAllRanges() {}, addRange(r) { state.selected = r.node.textContent; } }) };
globalThis.document = {
  addEventListener: (type, fn) => { listener = fn; },
  getElementById: (id) => ({ f: field, "jc-copy-status": status })[id] ?? null,
  createElement: () => (area = { value: "", style: {}, setAttribute() {}, select() { state.picked = this.value; }, remove() {} }),
  createRange: () => ({ selectNodeContents(node) { this.node = node; } }),
  body: { appendChild() {} },
  execCommand: (cmd) => { state.execs.push(cmd); if (%s === "throw") throw new Error("no"); if (%s) state.clipboard = state.picked; return %s === true; },
};
Object.defineProperty(globalThis, "navigator", { value: %s === "missing" ? {} : { clipboard: { writeText: (t) =>
  %s === "refuse" ? Promise.reject(new DOMException("denied", "NotAllowedError")) : (state.clipboard = t, Promise.resolve()) } },
  configurable: true });
globalThis.setTimeout = () => 0;
globalThis.clearTimeout = () => {};
%s
listener({ target: { closest: () => button } });
await new Promise((resolve) => setImmediate(resolve));
state.label = label.textContent;
console.log(JSON.stringify(state));
""" % (json.dumps(field_value), json.dumps(field_value), json.dumps(text), json.dumps(text), json.dumps(exec_copy),
       json.dumps(exec_copy), json.dumps(exec_copy), json.dumps(clipboard), json.dumps(clipboard), script)
    done = subprocess.run(["node", "--input-type=module", "-e", code], capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    return json.loads(done.stdout)


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_a_copy_button_puts_its_one_line_text_on_the_clipboard_and_says_so():
    """T-4762: the Claude Code button copies the one-line command, never the card's wrapped block."""
    line = "claude mcp add --transport http joinedcontext https://api.x/api/mcp"
    out = _copy_click("works", False, text=line)
    assert out["clipboard"] == line and out["execs"] == []
    assert out["status"] == "Skopírované" and out["label"] == "Skopírované"
    # A field without its own text copies the field's value.
    assert _copy_click("works", False, field_value="https://api.x/api/mcp")["clipboard"] == "https://api.x/api/mcp"


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
@pytest.mark.parametrize("clipboard", ["refuse", "missing"])
def test_a_refused_clipboard_falls_back_to_the_copy_command(clipboard):
    """A frame without clipboard-write or an http origin: the browser's copy command still copies."""
    out = _copy_click(clipboard, True, text='{"a": 1}')
    assert out["clipboard"] == '{"a": 1}' and out["execs"] == ["copy"]
    assert out["status"] == "Skopírované" and out["selected"] is None


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
@pytest.mark.parametrize("clipboard, exec_copy", [("refuse", False), ("missing", False), ("missing", "throw")])
def test_without_any_clipboard_the_text_is_selected_and_the_button_says_press_ctrl_c(clipboard, exec_copy):
    """No silent failure: the block is selected for Ctrl+C and the button and screen reader say so."""
    out = _copy_click(clipboard, exec_copy, text="one line")
    assert out["clipboard"] is None
    assert out["selected"].startswith("claude mcp add")
    assert out["status"] == out["label"] == "Označené: stlačte Ctrl+C"
    field = _copy_click(clipboard, exec_copy, field_value="https://api.x/api/mcp")
    assert field["selected"] == "https://api.x/api/mcp"


def test_the_mcp_card_puts_claude_first_and_keeps_its_code_whole():
    """T-4284 (owner screenshot): the address and the code are never cut or broken mid-word, each
    copy button sits in its block's header, and the Claude connector steps come first."""
    panel = (THEME / "templates__snippets__jc_mcp_install.html").read_text()
    order = [panel.index(key) for key in ("mcp_claude_ai_1", "mcp_vscode", "mcp_cursor", "jc-mcp-claude-label", "jc-mcp-json-label")]
    assert order == sorted(order)
    assert panel.count('<div class="jc-code-head">') == 3
    assert "mcp.url_parts | join('<wbr>' | safe)" in panel
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".jc-live .jc-code pre { white-space: pre; overflow-wrap: normal; overflow-x: auto; }" in css
    assert ".jc-code .jc-code-url { overflow-wrap: normal; word-break: normal; }" in css
    # On /assistants the Claude card holds Claude's steps, so the block leaves them out (T-4585).
    assert panel.index("{% if not editors_only %}\n    <div>") < panel.index("mcp_claude_ai_1") < panel.index("{% endif %}\n    <div>\n    <h3 class=\"jc-mcp-client\">{{ h.jc_t('mcp_editors') }}")


def test_each_resource_has_its_one_action_named_for_a_screen_reader():
    item = (THEME / "templates__package__snippets__resource_item.html").read_text()
    assert "dropdown" not in item and "{{ _('Explore') }}" not in item
    # The publisher's own page opens; only a file downloads (T-4333).
    assert "h.jc_t('open') if jc_section in ('api', 'source') else h.jc_t('download')" in item
    assert item.count('<span class="visually-hidden"> {{ name }}</span>') == 3
    assert "{% if not url_is_edit %}" in item and "jc_format or h.get_translated(res, 'format')" in item


@pytest.mark.parametrize("lang, words", [
    ("sk", ("Vydavateľ", "Denne")), ("cs_CZ", ("Vydavatel", "Denně")), ("de", ("Herausgeber", "Täglich")), ("hu", ("Kiadó", "Naponta")), ("fi", ("Publisher", "Every day")),
])
def test_the_page_language_picks_the_words_and_an_unknown_one_reads_english(theme, lang, words):
    theme.page.lang = lang
    rows = dict((label, text) for label, text, _ in theme.jc_about(PUBLISHED))
    assert words[0] in rows and rows[words[0]] == "Environment Services"
    assert words[1] in rows.values()


def test_every_word_of_the_theme_is_in_every_language(theme):
    """T-5115: the CI check that fails on a missing key, empty text or an untranslated placeholder."""
    tables = {"STRINGS": theme.STRINGS, "FORMATS": theme.FORMATS, "MODEL_FILES": theme.MODEL_FILES,
              "EXTRA_LABELS": theme.EXTRA_LABELS, "NUTS_NAMES": theme.NUTS_NAMES, "TOPICS": theme.TOPICS,
              "FREQUENCIES": {key: words for key, (words, _) in theme.FREQUENCIES.items()}}
    for name, table in tables.items():
        for key, words in table.items():
            assert set(words) == LANGUAGES, (name, key)
            assert all(words.values()), (name, key)
            placeholders = set(re.findall(r"\{\w+\}", str(words["en"])))
            for lang, text in words.items():
                assert set(re.findall(r"\{\w+\}", str(text))) == placeholders, (name, key, lang)


def test_no_word_of_the_theme_is_defined_twice():
    """T-5115: a second "guide" entry silently replaced the WebSocket guide's link text with "Guide"."""
    import ast

    tree = ast.parse((THEME / "plugin.py").read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and isinstance(node.value, ast.Dict):
            keys = [k.value for k in node.value.keys if isinstance(k, ast.Constant)]
            assert len(keys) == len(set(keys)), (node.targets[0].id, sorted({k for k in keys if keys.count(k) > 1}))


def test_the_websocket_guide_and_the_assistant_guide_keep_their_own_words(theme):
    theme.page.lang = "en"
    assert theme.jc_t("guide") == "The WebSocket guide: messages, limits and examples"
    assert theme.jc_t("guide_open") == "Guide"
    theme.page.lang = "hu"
    assert theme.jc_t("guide_open") == "Útmutató" and theme.jc_t("tagline").startswith("Böngéssze")


def test_the_search_filters_are_named_in_plain_words(theme):
    facets = {"organization": "Organizations", "groups": "Groups", "tags": "Tags", "res_format": "Formats", "license_id": "Licenses"}
    renamed = theme.JcThemePlugin().dataset_facets(dict(facets), "dataset")
    # T-5294: Topic is the EU theme; T-5649: Format the served formats, in the places of CKAN's groups and file formats.
    assert list(renamed.items()) == [("organization", "Publisher"), ("vocab_jc_topic", "Topic"), ("tags", "Keywords"),
                                     ("vocab_jc_format", "Format"), ("license_id", "Licence")]


THEMES = "http://publications.europa.eu/resource/authority/data-theme/"
# What `jcctl` publishes for an Endpoint whose outputs are all served (dev, 2026-10-25).
SERVED = PUBLISHED_RESOURCES + [
    {"id": "r-ws", "format": "WebSocket", "url": ENDPOINT + "ws"},
    {"id": "r-ogc", "format": "OGCFEAT", "url": ENDPOINT + "ogc/features/"},
    {"id": "r-odata", "format": "OData", "url": ENDPOINT + "odata/v4/"},
    {"id": "r-sta", "format": "STA", "url": ENDPOINT + "sta/v1.1/"},
    {"id": "r-dcat", "format": "JSON-LD", "url": ENDPOINT},
    {"id": "r-ics", "format": "ICS", "mimetype": "text/calendar", "url": ENDPOINT + "ical/v1/zasadnutia.ics"},
    {"id": "r-jf", "format": "JSON", "mimetype": "application/feed+json", "url": ENDPOINT + "feed/v1/kultura/feed.json"},
]


def test_a_dataset_is_indexed_by_the_formats_it_serves_and_never_by_its_model_documents(theme):
    """T-5649: dev's Format filter offered the uses Map and Live feed, translated; the owner wants the formats."""
    assert theme.jc_data_formats({"resources": PUBLISHED_RESOURCES}) == ["NGSI-LD", "CSV", "MCP"]
    assert theme.jc_data_formats({"resources": SERVED}) == [
        "NGSI-LD", "WEBSOCKET", "OGCFEAT", "ODATA", "STA", "CSV", "ICS", "JSONFEED", "MCP"]
    # The schema index is JSON under /schema/: a model document, like SHACL and the JSON-LD context.
    model = [r for r in PUBLISHED_RESOURCES if r["id"] in ("r-index", "r-shacl", "r-ctx")]
    assert theme.jc_data_formats({"resources": model}) == [] and theme.jc_data_formats({}) == []
    # A format the Endpoint does not serve has no resource, so it is no value: no WebSocket here.
    assert "WEBSOCKET" not in theme.jc_data_formats({"resources": PUBLISHED_RESOURCES})
    # A harvested file off the platform counts by its format; the link to the publisher's page does not.
    harvested = {"title": "Trees", "resources": [{"format": "GeoJSON", "url": "https://other.example/a.geojson"},
                                                 {"name": "Trees", "format": "HTML", "url": "https://other.example/trees"}]}
    assert theme.jc_data_formats(harvested) == ["GEOJSON"]
    assert theme.jc_data_formats({"resources": [{"format": "PDF", "url": "https://other.example/a.pdf"}]}) == []


def test_a_card_names_the_formats_by_their_standard_names_in_every_language(theme):
    chips = theme.jc_format_chips({"resources": SERVED})
    assert chips == ["NGSI-LD", "WebSocket", "OGC API Features", "OData", "SensorThings API", "CSV", "iCalendar",
                     "JSON Feed", "MCP"]
    theme.page.lang = "sk"
    assert theme.jc_format_chips({"resources": SERVED}) == chips


def test_the_themes_are_indexed_by_eu_code_and_an_unknown_one_is_left_out(theme):
    pkg = {"extras": [{"key": "theme", "value": json.dumps([THEMES + "ENVI", THEMES + "HEAL", THEMES + "ENVI",
                                                           THEMES + "OP_DATPRO", "https://other.example/x"])}]}
    assert theme.jc_topics(pkg) == ["ENVI", "HEAL"]
    assert theme.jc_topics({"extras": [{"key": "theme", "value": "not json"}]}) == []
    assert theme.jc_topics({"extras": [{"key": "theme", "value": '{"a": 1}'}]}) == []
    assert theme.jc_topics({}) == []


def test_the_index_carries_the_formats_and_the_topics_of_the_stored_dataset(theme):
    stored = {"resources": PUBLISHED_RESOURCES, "extras": [{"key": "theme", "value": json.dumps([THEMES + "TRAN"])}]}
    indexed = theme.JcThemePlugin().before_dataset_index({"id": "d", "data_dict": json.dumps(stored)})
    assert indexed["vocab_jc_format"] == ["NGSI-LD", "CSV", "MCP"] and indexed["vocab_jc_topic"] == ["TRAN"]
    assert theme.JcThemePlugin().before_dataset_index({"id": "e"})["vocab_jc_format"] == []


def _results(count, **facets):
    return {"count": count, "results": [], "search_facets": {
        field: {"title": field, "items": [{"name": n, "display_name": n, "count": c} for n, c in items]}
        for field, items in facets.items()}}


def test_the_filters_name_formats_untranslated_and_topics_in_the_page_language(theme):
    theme.page.lang = "sk"
    results = _results(140, vocab_jc_format=[("NGSI-LD", 140), ("OGCFEAT", 136), ("WEBSOCKET", 95), ("STA", 3)],
                       vocab_jc_topic=[("ECON", 21), ("TRAN", 21), ("REGI", 140)], tags=[("Praha", 140)])
    out = theme.JcThemePlugin().after_dataset_search(results, {"fq": ""})["search_facets"]
    # T-5649: every served format stays, NGSI-LD on all 140 included, under its own name.
    assert [(i["name"], i["display_name"], i["count"]) for i in out["vocab_jc_format"]["items"]] == [
        ("NGSI-LD", "NGSI-LD", 140), ("OGCFEAT", "OGC API Features", 136), ("WEBSOCKET", "WebSocket", 95),
        ("STA", "SensorThings API", 3)]
    # A topic on every result narrows nothing (T-5294).
    assert [i["display_name"] for i in out["vocab_jc_topic"]["items"]] == ["Hospodárstvo a financie", "Doprava"]
    # Keywords are CKAN's own and stay as they are.
    assert out["tags"]["items"] == [{"name": "Praha", "display_name": "Praha", "count": 140}]


def test_a_value_the_search_filters_by_stays_so_it_can_be_taken_off(theme):
    results = _results(98, vocab_jc_topic=[("ECON", 98), ("TRAN", 98), ("ENVI", 50)])
    out = theme.JcThemePlugin().after_dataset_search(results, {"fq": 'vocab_jc_topic:"TRAN" +capacity:public'})
    assert [i["name"] for i in out["search_facets"]["vocab_jc_topic"]["items"]] == ["TRAN", "ENVI"]
    # CKAN 2.11's query hands the hook `fq` as a list of clauses (lib/search/query.py), and fq_list beside it.
    for params in ({"fq": ['+capacity:public vocab_jc_topic:"TRAN"', "+site_id:x"]},
                   {"fq": ["+site_id:x"], "fq_list": ['vocab_jc_topic:"TRAN"']}):
        results = _results(98, vocab_jc_topic=[("ECON", 98), ("TRAN", 98), ("ENVI", 50)])
        out = theme.JcThemePlugin().after_dataset_search(results, params)
        assert [i["name"] for i in out["search_facets"]["vocab_jc_topic"]["items"]] == ["TRAN", "ENVI"], params


def test_the_home_page_tiles_name_each_theme_with_its_count_and_search(theme, monkeypatch):
    search_module = types.ModuleType("ckan.lib.search")
    search_module.SearchError = type("SearchError", (Exception,), {})
    monkeypatch.setitem(sys.modules, "ckan.lib", types.ModuleType("ckan.lib"))
    monkeypatch.setitem(sys.modules, "ckan.lib.search", search_module)
    asked = []

    def search(data_dict):
        asked.append(data_dict)
        return {"facets": {"vocab_jc_topic": {"TRAN": 18, "ECON": 21, "SOCI": 0, "XXXX": 3}}}
    theme.page.actions["package_search"] = search
    monkeypatch.setattr(theme.toolkit, "url_for", lambda route, **kw: "/dataset/?vocab_jc_topic=" + kw["vocab_jc_topic"],
                        raising=False)
    tiles = theme.jc_topic_tiles()
    assert [(t["code"], t["label"], t["count"], t["url"]) for t in tiles] == [
        ("ECON", "Economy and finance", 21, "/dataset/?vocab_jc_topic=ECON"),
        ("TRAN", "Transport", 18, "/dataset/?vocab_jc_topic=TRAN")]
    assert asked[0]["rows"] == 0 and asked[0]["fq"] == 'capacity:"public"'
    assert theme.jc_rows(1, "datasets_n") == "1 dataset" and theme.jc_rows(21, "datasets_n") == "21 datasets"
    theme.page.lang = "sk"
    assert theme.jc_rows(3, "datasets_n") == "3 datasety" and theme.jc_rows(5, "datasets_n") == "5 datasetov"
    theme.page.actions["package_search"] = lambda data_dict: {"facets": {}}
    assert theme.jc_topic_tiles() == []
    theme.page.actions["package_search"] = search_module.SearchError("solr down")
    assert theme.jc_topic_tiles() == []


def test_a_search_result_and_a_home_card_show_the_served_formats_and_no_model_document():
    for name in ("templates__snippets__package_item.html", "templates__home__index.html"):
        text = (THEME / name).read_text()
        assert "h.jc_format_chips(" in text and "h.jc_formats(" not in text, name


def test_the_portal_link_is_on_the_installation_s_domain(theme):
    assert theme.jc_portal_url() == "https://portal.dev.example/"


def test_every_page_template_and_static_file_is_mounted_at_its_path(rendered):
    (ckan,) = [d for d in rendered("dev") if d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("ckan")
               and any(c["name"] == "ckan" for c in d["spec"]["template"]["spec"]["containers"])]
    (container,) = [c for c in ckan["spec"]["template"]["spec"]["containers"] if c["name"] == "ckan"]
    mounts = {m["subPath"]: m["mountPath"] for m in container["volumeMounts"] if m["name"] == "theme"}
    files = [p.name for p in THEME.iterdir() if p.name.startswith(("templates__", "public__"))]
    assert files, "the theme has page templates"
    for name in files:
        assert mounts[name] == "/srv/theme/ckanext_jc_theme/" + name.replace("__", "/"), name
    assert "__init__.py" in mounts and mounts["__init__.py"].endswith("ckanext_jc_theme/__init__.py")


def test_the_theme_loads_nothing_from_another_origin():
    for path in THEME.iterdir():
        if path.suffix == ".py":
            continue
        text = path.read_text()
        # `url(#id)` names a gradient or mask in the same SVG (the T-4613 logos) and loads nothing.
        assert not re.search(r"<script>|(src|href)=\"(https?:)?//|(?<!\w)url\((?!#)|@import", text), path.name


# --- T-2888: the catalogue wears the installation's look, and a failed sign-in says why ---------


def test_the_brand_colour_reaches_bootstrap_and_its_text_stays_legible(theme, tmp_path):
    assert theme.BRANDING["primaryRgb"] == "29, 78, 216"
    assert theme.BRANDING["primaryForeground"] == "#ffffff"
    light = tmp_path / "light.json"
    light.write_text(json.dumps({"colours": {"primary": "#ffe977"}}))
    theme.BRANDING_FILE = str(light)
    block = theme._load()
    assert block["primaryRgb"] == "255, 233, 119"
    # White on a pale yellow is unreadable; the login theme's YIQ rule gives black.
    assert block["primaryForeground"] == "#000000"
    short = tmp_path / "short.json"
    short.write_text(json.dumps({"colours": {"primary": "#00f"}}))
    theme.BRANDING_FILE = str(short)
    assert theme._load()["primaryRgb"] == "0, 0, 255"


STOCK_TEALS = r"#206b82|#187794|#1a5668|#005d7a|#003647|#00232e|#bfd7de|#d9e7eb|#000f14"


def test_no_stock_ckan_colour_survives_the_theme():
    """CKAN compiles its teal into its own rules; each group of them is restated on the brand."""
    css = (THEME / "public__jc-theme.css").read_text()
    for variable in ("--bs-primary: var(--jc-primary)", "--bs-primary-rgb: var(--jc-primary-rgb)",
                     "--bs-link-color: var(--jc-primary)"):
        assert variable in css, variable
    for rule in (".btn-link, .link-primary", ".btn-primary:hover", ".form-check-input:checked",
                 ".form-control:focus", ".masthead .main-navbar ul li:hover a", ".view-list li a.active .icon",
                 ".account-masthead {", ".dropdown-item.active", ".page-item.active .page-link",
                 "body.dt-view .page-item.active .page-link"):
        assert rule in css, rule
    for path in THEME.iterdir():
        if path.suffix in (".html", ".css"):
            assert not re.search(STOCK_TEALS, path.read_text(), re.I), path.name


def test_the_stylesheet_carries_no_colour_of_its_own():
    """T-3009: every colour is the branding block's or mixed from it, so values restyle it all."""
    css = (THEME / "public__jc-theme.css").read_text()
    # T-4620: the three assistant segments wear their makers' colours, which no instance restyles
    css, makers = re.subn(r"^\.jc-seg-(?:claude|chatgpt|mcp) \{ background: [^}]*\}$", "", css, flags=re.M)
    assert makers == 3
    assert not re.search(r"#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(", css, re.I)
    named = re.findall(r":\s*(white|black|red|blue|green|gray|grey|navy|teal)\b", css, re.I)
    assert not named, named
    styles = (THEME / "templates__snippets__jc_styles.html").read_text()
    for token in ("--jc-primary:", "--jc-primary-fg:", "--jc-primary-rgb:", "--jc-secondary:", "--jc-accent:",
                  "--jc-background:", "--jc-text:", "--jc-font-heading:", "--jc-font-body:"):
        assert token in styles, token
    assert "h.url_for_static('/jc-theme.css')" in styles


def test_the_table_view_wears_the_theme_and_the_theme_s_templates_win():
    """The table view's own template empties the styles block; the theme restores it there, and
    only the first plugin's templates outrank the table view's."""
    view = (THEME / "templates__datatables__datatables_view.html").read_text()
    assert "{% ckan_extends %}" in view and "snippets/jc_styles.html" in view
    assert "snippets/jc_styles.html" in (THEME / "templates__base.html").read_text()
    values = yaml.safe_load((PROJECT_ROOT / "charts/ckan/values.yaml").read_text())
    assert values["ckan"]["plugins"].split()[0] == "jc_theme"


def test_the_home_page_replaces_ckan_s_sample_page():
    home = (THEME / "templates__home__index.html").read_text()
    assert "{% block primary_content %}" in home
    for part in ("h.jc_site_tagline()", "h.get_site_statistics()", "h.jc_recent(6)",
                 "get_facet_items_dict('organization'", 'role="search"', '<label for="jc-hero-q">'):
        assert part in home, part
    assert "home/snippets/" not in home


def test_tagline_and_footer_lines_are_read_as_words_and_nothing_else(theme, tmp_path):
    assert theme.BRANDING["tagline"] == "" and theme.BRANDING["footerLines"] == []
    block = tmp_path / "words.json"
    block.write_text(json.dumps({"tagline": "Open data of the city",
                                 "footerLines": ["Demo instance, not affiliated.", "", "  ", 7, None]}))
    theme.BRANDING_FILE = str(block)
    loaded = theme._load()
    assert loaded["tagline"] == "Open data of the city"
    assert loaded["footerLines"] == ["Demo instance, not affiliated."]
    block.write_text(json.dumps({"tagline": {"x": 1}, "footerLines": "not a list"}))
    loaded = theme._load()
    assert loaded["tagline"] == "" and loaded["footerLines"] == []


def test_a_demo_ribbon_and_noindex_come_from_the_branding_block_alone(theme, tmp_path):
    """T-4687: a demo says so on every page and keeps search engines out; nothing else does."""
    assert theme.BRANDING["ribbon"] == "" and theme.BRANDING["noindex"] is False
    block = tmp_path / "demo.json"
    block.write_text(json.dumps({"ribbon": "  DEMO · ukážka  ", "noindex": True}))
    theme.BRANDING_FILE = str(block)
    loaded = theme._load()
    assert loaded["ribbon"] == "DEMO · ukážka" and loaded["noindex"] is True
    block.write_text(json.dumps({"ribbon": ["DEMO"], "noindex": "yes"}))
    loaded = theme._load()
    assert loaded["ribbon"] == "" and loaded["noindex"] is False
    header = (THEME / "header.html").read_text()
    assert '{% if ribbon %}<div class="jc-ribbon" role="note">{{ ribbon }}</div>{% endif %}' in header
    assert '<meta name="robots" content="noindex, nofollow">' in (THEME / "templates__base.html").read_text()
    assert ".jc-ribbon { position: sticky; top: 0;" in (THEME / "public__jc-theme.css").read_text()


def test_the_footer_carries_the_installations_links_and_none_of_ckans():
    footer = (THEME / "footer.html").read_text()
    assert "{% block footer_links_ckan %}{% endblock %}" in footer
    for stock in ("docs.ckan.org", "ckan.org", "opendefinition", "od_80x15"):
        assert stock not in footer, stock
    assert "h.jc_t('about_site')" in footer and "h.jc_portal_url()" in footer
    # The demo disclaimer stays: the organisation line, and the footer lines of the block.
    assert "brand.organisation" in footer and "brand.footerLines" in footer


def test_the_footers_api_link_opens_the_page_that_explains_the_api_not_raw_json():
    """T-4234 (photo audit T-4161 #12): "Catalogue API" opened package_list's raw JSON with no
    word on what the API offers. It opens the About page's developer section, which names the
    address, says what it returns and links the JSON one step further."""
    footer = (THEME / "footer.html").read_text()
    assert "<a href=\"{{ h.url_for('home.about') }}#api\">{{ h.jc_t('catalogue_api') }}</a>" in footer
    assert "logic_function='package_list'" not in footer
    about = (THEME / "templates__home__about.html").read_text()
    assert '<h2 id="api">{{ h.jc_t(\'about_machines\') }}</h2>' in about
    assert "logic_function='package_list'" in about and "h.jc_t('about_api')" in about


def test_a_second_deployment_restyles_the_catalogue_from_values_alone(rendered, rendered_variant):
    """T-3009: a new palette, logo, tagline and footer reach the theme with no file of it edited."""
    logo = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1" fill="#8a1538"/></svg>'

    def rebrand(tree):
        env = tree / "deployment/environments/dev/global.yaml.gotmpl"
        text = env.read_text()
        assert "primary: '#0000bf'" in text
        text = text.replace("primary: '#0000bf'", "primary: '#8a1538'")
        text = text.replace("    logo: logo.svg\n", "    logo: logo.svg\n    tagline: Otvorené dáta mesta\n"
                            "    footerLines:\n      - Demo, not affiliated.\n", 1)
        env.write_text(text)
        (tree / "deployment/environments/dev/branding/logo.svg").write_text(logo + "\n")

    def theme_of(docs):
        (cm,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "ckan-theme"]
        (ckan,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "ckan"]
        return cm["data"], ckan["spec"]["template"]["metadata"]["annotations"]["checksum/theme"]

    before, before_sum = theme_of(rendered("dev"))
    after, after_sum = theme_of(rendered_variant("dev", rebrand))
    branding = json.loads(after["branding.json"])
    assert branding["colours"]["primary"] == "#8a1538"
    assert branding["tagline"] == "Otvorené dáta mesta" and branding["footerLines"] == ["Demo, not affiliated."]
    assert after["logo.svg"].strip() == logo
    # The theme's own files are byte for byte the same: only the values changed.
    assert {k: v for k, v in after.items() if k not in ("branding.json", "logo.svg")} == \
        {k: v for k, v in before.items() if k not in ("branding.json", "logo.svg")}
    # And the pod restarts to show it.
    assert after_sum != before_sum


@pytest.mark.parametrize("reason, key", [
    ("Login process was not started properly", "sso_cookie"),
    ("The app state does not match", "sso_state"),
    ("Unsupported token type. Should be 'Bearer'.", "sso_refused"),
    ("No access token returned from the token endpoint.", "sso_refused"),
    ("Unique user not found", "sso_account"),
    ("access_denied", "sso_other"),
])
def test_a_failed_sign_in_says_why_once_in_words_a_person_acts_on(theme, reason, key):
    session = {theme.SSO_ERROR: reason}
    assert theme.jc_sso_error(session) == theme.STRINGS[key]["en"]
    # Said once: a reload of the login page does not repeat an old failure.
    assert theme.jc_sso_error(session) is None
    theme.page.lang = "sk"
    session[theme.SSO_ERROR] = reason
    assert theme.jc_sso_error(session) == theme.STRINGS[key]["sk"]


def test_a_sign_in_that_did_not_fail_says_nothing(theme):
    assert theme.jc_sso_error({}) is None


def test_the_login_page_shows_the_reason_as_an_alert():
    login = (THEME / "templates__user__login.html").read_text()
    assert "h.jc_sso_error()" in login and 'role="alert"' in login


# --- T-3028: the About page, the publisher and topic pages speak for this catalogue ------------


def test_the_about_page_says_whose_catalogue_this_is_and_how_to_reach_the_data(theme, tmp_path):
    block = tmp_path / "about.json"
    block.write_text(json.dumps({"instanceName": "Praha Context", "organisation": "Hlavní město Praha",
                                 "contactEmail": "opendata@praha.eu"}))
    theme.BRANDING_FILE = str(block)
    theme.BRANDING = theme._load()
    theme.page.loaded = {"dcat"}
    about = theme.jc_about_site()
    assert about == {"intro": "Praha Context is the open data catalogue of Hlavní město Praha.",
                     "dcat": True, "email": "opendata@praha.eu"}
    theme.page.lang = "de"
    assert theme.jc_about_site()["intro"] == "Praha Context ist der Katalog offener Daten von Hlavní město Praha."


def test_an_about_page_without_organisation_contact_or_dcat_leaves_those_out(theme, tmp_path):
    block = tmp_path / "bare.json"
    block.write_text(json.dumps({"instanceName": "City Context", "contactEmail": "not an address"}))
    theme.BRANDING_FILE = str(block)
    theme.BRANDING = theme._load()
    assert theme.jc_about_site() == {"intro": "City Context is an open data catalogue.", "dcat": False, "email": ""}
    about = (THEME / "templates__home__about.html").read_text()
    assert "{% if about.dcat %}" in about and "{% if about.email %}" in about


def test_a_sysadmin_s_own_about_text_still_wins_and_no_ckan_advert_is_left():
    about = (THEME / "templates__home__about.html").read_text()
    assert "{% if g.site_about %}\n    {{ super() }}" in about
    assert "about_text.html" not in about and "ckan.org" not in about
    for name in ("templates__organization__snippets__helper.html", "templates__group__snippets__helper.html"):
        helper = (THEME / name).read_text()
        assert "CKAN" not in helper.split("#}", 1)[1] and "h.jc_t(" in helper, name


def test_the_header_names_the_sections_as_the_search_filters_do():
    header = (THEME / "header.html").read_text()
    for key in ("nav_datasets", "nav_publishers", "nav_topics", "nav_about"):
        assert f"h.jc_t('{key}')" in header, key
    assert "_('Organizations')" not in header and "_('Groups')" not in header


def test_the_header_class_is_never_reused_for_icons():
    """T-4627: T-4613 gave the corner's AI logos the header's `.jc-brand`, whose 22px block and
    skew then hit the header anchor and stacked the mark over the name. `.jc-brand` is the header
    link alone: no rule for it sizes, blocks or skews it."""
    css = (THEME / "public__jc-theme.css").read_text()
    for selectors, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css):
        names = [s.strip() for s in selectors.split(",")]
        if any(re.fullmatch(r"\.jc-brand(:[\w-]+)*", n) for n in names):
            for banned in ("inline-size", "display: block", "transform"):
                assert banned not in body, (selectors.strip(), banned)
    for mark in ("claude", "chatgpt", "mcp"):
        assert 'class="jc-ai-logo"' in (THEME / f"templates__jc__brand-{mark}.svg").read_text(), mark


def test_the_header_is_the_joinedcontext_lockup_named_by_the_site_title():
    """T-4627: one image, the joinedcontext.com lockup copied into the chart, its alt the
    installation's name; no span beside it."""
    jinja2 = pytest.importorskip("jinja2")
    header = (THEME / "header.html").read_text()
    block = header.split("{% block header_logo %}", 1)[1].split("{% endblock %}", 1)[0]
    html = jinja2.Environment(autoescape=True).from_string(block).render(h=types.SimpleNamespace(
        url_for=lambda name: "/", jc_site_logo=lambda: "/jc-logo.svg", jc_site_title=lambda: "Otvorené dáta"))
    imgs = re.findall(r"<img [^>]*>", html)
    assert imgs == ['<img src="/jc-logo.svg" alt="Otvorené dáta" width="171" height="36">'], imgs
    assert "<span" not in html
    logo = (THEME / "public__jc-logo.svg").read_text()
    assert logo.startswith('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 266 56"')
    assert not re.search(r"<script|\son\w+=|href=|<image|<foreignObject", logo, re.I)
    assert set(re.findall(r"url\(([^)]*)\)", logo)) <= {f"#{i}" for i in re.findall(r'id="([^"]+)"', logo)}


def test_publisher_and_topic_cards_are_a_grid_that_wraps_at_words():
    css = (THEME / "public__jc-theme.css").read_text()
    assert re.search(r"\.media-grid \{ clear: both; display: grid;", css)
    assert ".media-grid > .clearfix { display: none; }" in css
    assert "word-break: normal" in css and "float: none; width: auto" in css
    for name in ("templates__organization__snippets__organization_list.html", "templates__group__snippets__group_list.html"):
        listing = (THEME / name).read_text()
        # CKAN's media-grid module places each card absolutely (masonry) over the grid.
        assert 'data-module="media-grid"' not in listing and '<ul class="media-grid">' in listing, name


def test_a_short_page_keeps_its_footer_at_the_bottom():
    css = (THEME / "public__jc-theme.css").read_text()
    assert "body:not(.dt-view) { min-height: 100vh; display: flex; flex-direction: column; }" in css
    assert "body:not(.dt-view) > .site-footer { margin-top: auto; }" in css


# T-3048: what a reader meets on the resource page, the search filters and a narrow table.

REGISTRE = {"name": "bbsk-registre", "title": "Registre kraja"}


@pytest.mark.parametrize("lang, shacl, owl", [
    ("en", "Validation rules", "OWL"),
    ("sk", "Pravidlá kontroly", "OWL"),
    ("fi", "Validation rules", "OWL"),
])
def test_a_data_model_file_is_named_in_words_without_the_dataset_s_name(theme, lang, shacl, owl):
    theme.page.lang = lang
    label = theme.jc_resource_label
    assert label({"name": "bbsk-registre model.shacl.ttl"}, REGISTRE) == shacl
    assert label({"name": "bbsk-registre model.owl.ttl"}, REGISTRE) == owl
    # Every file jcctl publishes for a model reads as words, none as a file name.
    names = {label({"name": f"bbsk-registre {file}"}, REGISTRE) for file in theme.MODEL_FILES}
    assert len(names) == len(theme.MODEL_FILES) and not any("bbsk-registre" in n for n in names)


def test_another_resource_keeps_its_name_and_an_empty_one_falls_back(theme):
    label = theme.jc_resource_label
    assert label({"name": "Bridge"}, REGISTRE) == "Bridge"
    assert label({"name": "bbsk-registre context.jsonld"}, None) == "bbsk-registre context.jsonld"
    assert label({"name": "bbsk-registre"}, REGISTRE) == "bbsk-registre"  # nothing after the prefix
    assert label({"name": "", "format": "CSV"}, REGISTRE) == "CSV"
    assert label({"id": "r1"}, REGISTRE) == "r1"
    assert label(None, None) == ""


def test_the_resource_list_and_page_name_resources_in_words_and_never_cut_them():
    sidebar = (THEME / "templates__package__snippets__resources.html").read_text()
    page = (THEME / "templates__package__resource_read.html").read_text()
    row = (THEME / "templates__package__snippets__resource_item.html").read_text()
    for template in (sidebar, page, row):
        assert "h.jc_resource_label(" in template
        assert "|truncate" not in template and "| truncate" not in template
    assert 'aria-current="page"' in sidebar
    crumb = page[page.index("{% block breadcrumb_content %}"):page.index("{% endblock %}", page.index("{% block breadcrumb_content %}"))]
    assert "h.jc_resource_label(res, pkg)" in crumb and "super()" not in crumb
    # One line back to the dataset, never its whole description again.
    assert "Dataset description" not in page and "h.jc_t('part_of')" in page


def test_a_file_without_a_view_says_what_it_is_and_offers_its_one_action():
    page = (THEME / "templates__package__resource_read.html").read_text()
    block = page[page.index("{% block resource_view_content %}"):]
    assert "_(\"There are no views" not in block
    # A file says what it is and offers Download; an API offers no download (T-4333).
    files = block[block.index("{% else %}", block.index("{% elif api %}")):]
    assert "h.jc_t('file_lead')" in files and "h.jc_t('download')" in files
    # Whoever may create views still gets CKAN's own page.
    assert "{{ super() }}" in block and "resource_view_create" in block


def test_a_search_filter_with_nothing_to_choose_is_left_out_and_labels_are_whole():
    facets = (THEME / "templates__snippets__facet_list.html").read_text()
    assert "_('There are no" not in facets
    assert "{% if items %}" in facets and "|truncate" not in facets
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".nav-facet .nav-item > a .item-label { overflow-wrap: anywhere; }" in css
    assert "text-overflow: ellipsis; white-space: nowrap; }\n" not in css.split(".nav-facet .nav-item > a .item-label", 1)[1][:80]
    # Owner, 2026-10-12: an empty footer drew a bordered strip under a short list (Licence), so the
    # footer is printed only inside the condition that gives it a link.
    guard = facets.index("{% if not limited or h.has_more_facets(name, search_facets) %}")
    assert guard < facets.index('<p class="module-footer">') < facets.index("{% endif %}\n      </section>")


def test_the_framed_table_shares_the_card_s_edge_one_border_and_one_row_of_controls():
    """T-4637 (was T-3048's stacking): the frame runs border to border with no border of its own,
    the view keeps the card's 1rem, controls and footer are one row each until the frame is
    narrow. The view's own rules name `#dtprv_wrapper`, so these do too, and the two margins that
    push search and pages right outrank the rule that zeroes every control's margin."""
    css = (THEME / "public__jc-theme.css").read_text()
    grid = next(line for line in css.splitlines() if line.startswith(".jc-grid {"))
    assert "margin-inline: -1rem; width: calc(100% + 2rem);" in grid and "border: 0;" in grid
    assert ".jc-table { border: 1px solid var(--jc-border);" in css and "padding: 0 1rem; }" in css
    assert "body.dt-view #resize_wrapper { inset: 0; }" in css
    assert "body.dt-view #dtprv_wrapper { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem 1rem; padding: 1rem; }" in css
    assert "body.dt-view #dtprv_wrapper > div.dataTables_length { order: 1; margin-inline-end: auto; }" in css
    assert "body.dt-view #dtprv_wrapper > div.dataTables_info { order: 5; padding: 0; margin-inline-end: auto;" in css
    assert "body.dt-view #dtprv_wrapper > .dataTables_scroll { order: 4; flex: 1 0 100%; }" in css
    for selector in (".dataTables_filter", ".dataTables_info", ".dataTables_paginate", ".dt-buttons"):
        lines = [line for line in css.splitlines() if "body.dt-view" in line and selector in line and "::" not in line]
        assert lines and all("#dtprv_wrapper" in line for line in lines), selector
    # The body holds its rows, so no band under two of them; its heights are inline.
    assert "body.dt-view #dtprv_wrapper .dataTables_scrollBody { height: auto !important; max-height: 26rem !important; }" in css
    narrow = css[css.index("@media (max-width: 47.99rem) {\n  body.dt-view"):]
    assert "> .dt-buttons { order: 0; flex: 1 0 100%; justify-content: flex-end; }" in narrow[:narrow.index("\n}\n")]
    # The summary's arrow hangs in the card's padding, so the table's name starts on the table's edge.
    assert '.jc-table > summary::before { content: "\\25B8"; position: absolute; inset-inline-start: -.75rem;' in css


def test_the_table_view_asks_for_no_missing_icon():
    """T-4637: the view's stylesheet names vendor/FontAwesome/images/times-circle-solid.svg for the
    search field's clear button, which the extension does not ship; the theme hides that button, so nothing asks for the icon."""
    css = (THEME / "public__jc-theme.css").read_text()
    assert 'body.dt-view input[type="search"]::-webkit-search-cancel-button { display: none; }' in css
    assert "times-circle" not in css


def test_the_frame_takes_the_height_its_view_says_from_the_same_origin_only():
    """T-4637: the view inside the frame posts its height; the page that frames it sets it on
    the frame the message came from, a number between 160 and 1280, same origin only."""
    script = (THEME / "public__jc-frame.js").read_text()
    assert 'postMessage({ type: TYPE, height: height }, window.location.origin)' in script
    assert "event.origin !== window.location.origin" in script and "contentWindow === event.source" in script
    assert "Math.min(Math.max(height, 160), 1280)" in script and "isFinite(height)" in script
    view = (THEME / "templates__datatables__datatables_view.html").read_text()
    assert "url_for_static('/jc-frame.js') }}\" defer></script>" in view
    for page in ("templates__package__read.html", "templates__package__resource_read.html"):
        assert "url_for_static('/jc-frame.js')" in (THEME / page).read_text(), page


# --- T-3058, AG-114: the catalogue offers its knowledge assistant ----------------------------


@pytest.mark.parametrize("address, offered", [
    ("https://assistant.dev.example/d/helsinki-catalogue/widget", True),
    ("", False),
    ("https://assistant.evil.example/d/helsinki-catalogue/widget", False),
    ("https://assistant.dev.example.evil.example/d/helsinki-catalogue/widget", False),
    ("http://assistant.dev.example/d/helsinki-catalogue/widget", False),
    ("https://assistant.dev.example/d/Helsinki/widget", False),
    ("https://assistant.dev.example/d/x/widget?next=//evil.example", False),
    ('https://assistant.dev.example/d/x/widget"><script>', False),
])
def test_the_widget_is_offered_only_from_the_installation_s_assistant_host(theme, address, offered):
    theme.ASSISTANT_WIDGET = address
    assert theme.jc_assistant_widget() == (address if offered else None)
    # A publisher's own address is held to the same rule (T-4281).
    theme.ASSISTANT_WIDGET = ""
    theme.ASSISTANT_WIDGETS = {"bbsk": address}
    assert theme.jc_assistant_widget("bbsk") == (address if offered else None)
    theme.ASSISTANT_WIDGETS = {}


def test_a_dataset_page_opens_its_publishers_assistant_and_any_other_page_the_catalogue_s(theme):
    """T-4281: the publisher's widget by CKAN organization, the catalogue's for every page with no
    publisher or a publisher with none of its own; an unreadable map is no map."""
    theme.ASSISTANT_WIDGET = "https://assistant.dev.example/d/bbsk/widget"
    theme.ASSISTANT_WIDGETS = {"praha": "https://assistant.dev.example/d/praha/widget"}
    assert theme.jc_assistant_widget("praha") == "https://assistant.dev.example/d/praha/widget"
    for organization in (None, "", "zilina"):
        assert theme.jc_assistant_widget(organization) == "https://assistant.dev.example/d/bbsk/widget"
    for raw in ("not json", "[1, 2]", '{"praha": 3}'):
        os.environ["JC_ASSISTANT_WIDGET_URLS"] = raw
        try:
            assert theme._widgets() == {}
        finally:
            del os.environ["JC_ASSISTANT_WIDGET_URLS"]
    theme.ASSISTANT_WIDGETS = {}


def test_the_footer_offers_the_assistant_as_a_disclosure_a_keyboard_opens():
    footer = (THEME / "footer.html").read_text()
    assert "{% set widget = h.jc_assistant_widget(publisher) %}" in footer
    assert "{% set publisher = pkg_dict.organization.name if pkg_dict is defined" in footer
    assert '<details class="jc-assistant" id="jc-assistant">' in footer and "<span>{{ h.jc_t('assistant') }}</span></summary>" in footer
    assert 'title="{{ h.jc_t(\'assistant\') }}"' in footer and 'loading="lazy"' in footer
    assert 'referrerpolicy="no-referrer"' in footer
    # The chat opens in the page's language (owner 2026-10-30, replacing T-4280).
    # T-4635: embed mode, so the chat's header carries the ✕ that closes the panel.
    assert '<iframe src="{{ widget }}?embed=1&amp;dock=1&amp;lang={{ h.lang() }}&amp;scheme=light"' in footer


def test_the_assistant_leaves_the_page_room_and_takes_a_phone_s_whole_screen():
    """T-4204: the button never covers the page's last line; open on a phone, the frame is the
    screen, the page behind neither shows nor takes focus, and Escape closes it."""
    css = (THEME / "public__jc-theme.css").read_text()
    assert "body:has(.jc-corner) { padding-block-end: 5rem; }" in css
    phone = css.split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    assert ".jc-assistant[open] { position: fixed; inset: 0;" in phone
    assert "body:has(.jc-assistant[open]) { overflow: hidden; }" in phone
    # The frame takes its size from the screen, the whole of it, with no bar above (T-4248, T-4635).
    assert ".jc-assistant[open] > iframe { position: fixed; inset: 0; width: 100%; height: 100vh; height: 100dvh;" in phone
    assert "3.25rem" not in phone
    footer = (THEME / "footer.html").read_text()
    assert "<script src=\"{{ h.url_for_static('/jc-assistant.js') }}\"></script>" in footer
    script = (THEME / "public__jc-assistant.js").read_text()
    assert 'matchMedia("(max-width: 40rem)")' in script and "el.inert = true" in script
    assert 'event.key === "Escape"' in script and 'querySelector("summary").focus()' in script


def test_the_open_assistant_has_one_close_control_and_a_frame_with_nothing_under_it():
    """T-4635 (owner's option C): open, the launcher is hidden on a desktop and a phone, and the ✕
    in the chat's header is the one close control; the frame sits in the corner the launcher left,
    1rem from the window's edge and clear of a phone's home bar, one tall height, 760px inside the
    window (T-4592)."""
    css = (THEME / "public__jc-theme.css").read_text()
    desktop = css.split("@media (max-width: 40rem) {", 1)[0]
    assert ".jc-assistant[open] > summary { display: none; }" in desktop
    frame = desktop.split(".jc-assistant > iframe { ", 1)[1].split("}", 1)[0]
    assert "position: fixed; inset-inline-end: 1rem; inset-block-end: max(1rem, env(safe-area-inset-bottom));" in frame
    assert "height: min(760px, calc(100vh - 2rem)); height: min(760px, calc(100dvh - 2rem));" in frame
    assert "100%" not in frame, "the frame no longer hangs above the launcher"
    assert "--jc-h" not in css
    phone = css.split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    assert ".jc-assistant[open] > summary" not in phone, "no bar on a phone either"
    assert "\\2715" not in css, "the page draws no ✕ of its own"
    # The chat's ✕ closes it: only the frame's own message, from the frame's origin, and focus
    # goes back to the launcher. No height is reported (T-4592).
    script = (THEME / "public__jc-assistant.js").read_text()
    assert "jc-assistant-height" not in script
    assert 'data.type === "jc-assistant-close"' in script
    heard = script.split('window.addEventListener("message", function (event) {', 1)[1]
    assert heard.index("event.source !== frame.contentWindow || event.origin !== origin") < heard.index('"jc-assistant-close"')
    assert 'box.querySelector("summary").focus();' in script


def test_the_chat_grows_to_the_window_on_its_own_frames_word_alone():
    """T-4658: the chat's expand button asks for the window; the page grows the frame only for its
    own frame's message from the frame's origin, to 60rem inside the 1rem gutter and the safe
    areas, and holds the page behind still. A phone keeps its whole-screen frame."""
    css = (THEME / "public__jc-theme.css").read_text()
    desktop = css.split("@media (max-width: 40rem) {", 1)[0]
    full = desktop.split(".jc-assistant.jc-full > iframe { ", 1)[1].split("}", 1)[0]
    assert "width: min(60rem, calc(100vw - 2rem));" in full
    assert "inset-block-start: max(1rem, env(safe-area-inset-top));" in full
    assert "height: calc(100dvh - max(1rem, env(safe-area-inset-top)) - max(1rem, env(safe-area-inset-bottom)));" in full
    assert "body:has(.jc-assistant.jc-full[open]) { overflow: hidden; }" in desktop
    assert css.index(".jc-assistant.jc-full > iframe") < css.index("@media (max-width: 40rem) {"), "the phone's frame wins"
    script = (THEME / "public__jc-assistant.js").read_text()
    size = script.split('data.type === "jc-assistant-size"', 1)
    assert len(size) == 2 and "event.source !== frame.contentWindow || event.origin !== origin" in size[0].rsplit("addEventListener(\"message\"", 1)[1]
    assert '(data.size === "full" || data.size === "panel" || data.size === "dock")' in size[1]
    assert 'phone.matches || box.classList.contains("jc-full")' in script


def test_the_assistant_docks_beside_the_page_and_stays_across_its_links():
    """T-4685 (API/05 §1.6.1): on a wide screen the open chat docks to the right edge and the
    page shrinks beside it; a link is a normal page load with the panel held by a view transition,
    open again before the first paint, on the assistant it had."""
    css = (THEME / "public__jc-theme.css").read_text()
    dock = css.split("@media (min-width: 1100px) {", 1)[1].split("\n}\n", 1)[0]
    # T-4767: docked, the body scrolls beside the frame and the window does not, so the page's bar is
    # not a second one beside the chat's at the screen's edge; a change of place keeps the offset.
    assert "html.jc-docked { --jc-dock-w: clamp(360px, var(--jc-dock, 34vw), 50vw); overflow: hidden; }" in dock
    assert "html.jc-docked body { margin-inline-end: var(--jc-dock-w); block-size: 100vh; block-size: 100dvh; overflow-y: auto; }" in dock
    held = (THEME / "public__jc-assistant.js").read_text().split("function place() {", 1)[1].split("\n  }\n", 1)[0]
    assert held.index("var top = scroller().scrollTop;") < held.index('root.classList.toggle("jc-docked"') < held.index("scroller().scrollTop = top;")
    assert "html.jc-docked .jc-assistant[open]:not(.jc-full) > iframe { inset-block: 0; inset-inline-end: 0; width: var(--jc-dock-w);" in dock
    assert "@view-transition { navigation: auto; }" in css
    assert ".jc-assistant[open] > iframe { view-transition-name: jc-assistant; }" in css
    assert css.index("@media (min-width: 1100px) {") < css.index("@media (max-width: 40rem) {"), "a phone's frame wins"
    reduced = css.split("@media (prefers-reduced-motion: reduce) {", 1)[1].split("\n}\n", 1)[0]
    assert "::view-transition-group(root)" in reduced and "animation: none;" in reduced
    base = (THEME / "templates__base.html").read_text()
    assert "<script src=\"{{ h.url_for_static('/jc-dock.js') }}\"></script>" in base
    # A failed jc-assistant.js (a CKAN rollout, owner report 05:55Z) gives the docked room back.
    early = (THEME / "public__jc-dock.js").read_text()
    guard = early.split('addEventListener("DOMContentLoaded"', 1)[1]
    assert "window.jcAssistant !== true" in guard and 'root.classList.remove("jc-docked")' in guard and "expect.remove()" in guard
    assert "window.jcAssistant = true;" in (THEME / "public__jc-assistant.js").read_text()
    assert 'expect.rel = "expect";' in early and 'expect.href = "#jc-assistant";' in early and 'setAttribute("blocking", "render")' in early
    assert "token" not in early.lower()
    script = (THEME / "public__jc-assistant.js").read_text()
    # The width: dragged or with the arrow keys, 360 px to half the window, kept per browser.
    assert 'setAttribute("role", "separator")' in script and "Math.max(px, 360)" in script and "window.innerWidth / 2" in script
    assert 'localStorage.setItem(WIDTH' in script and 'sessionStorage.setItem(KEY' in script
    # The kept assistant is one of this host's widget pages, never another address.
    ours = script.split("function ours(url) {", 1)[1].split("\n  }\n", 1)[0]
    assert "to.origin === origin" in ours and "\\/widget$/" in ours and "!to.search && !to.hash" in ours


def test_a_done_answer_opens_its_dataset_on_this_catalogue_alone():
    """T-4685: the chat names a dataset; the page opens it only on its own origin, a dataset path,
    not the page already open, and not while the visitor selects or types, and says so to the
    chat first. Only its own frame, from the frame's origin, is heard."""
    script = (THEME / "public__jc-assistant.js").read_text()
    opens = script.split("function opens(url) {", 1)[1].split("\n  }\n", 1)[0]
    assert "to.origin !== location.origin" in opens
    assert "/^\\/dataset\\/[a-z0-9_-]{2,100}\\/?$/.test(to.pathname)" in opens
    assert "=== here" in opens and "getSelection" in opens and "INPUT|TEXTAREA|SELECT" in opens
    heard = script.split('window.addEventListener("message", function (event) {', 1)[1]
    assert heard.index("event.source !== frame.contentWindow || event.origin !== origin") < heard.index('"jc-assistant-dataset"')
    dataset = heard.split('"jc-assistant-dataset"', 1)[1].split("} else if", 1)[0]
    assert dataset.index('postMessage({ type: "jc-assistant-opened", url: data.url }, origin)') < dataset.index("location.assign(path)")
    assert 'data.type === "jc-assistant-back"' in heard and "history.back()" in heard


def test_the_catalogue_pod_is_told_its_assistant_only_when_the_assistant_is_deployed(rendered, rendered_variant):
    def widget(docs):
        (ckan,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("ckan")
                   and any(c["name"] == "ckan" for c in d["spec"]["template"]["spec"]["containers"])]
        (container,) = [c for c in ckan["spec"]["template"]["spec"]["containers"] if c["name"] == "ckan"]
        env = {e["name"]: e.get("value") for e in container["env"]}
        return env["JC_ASSISTANT_WIDGET_URL"], json.loads(env["JC_ASSISTANT_WIDGET_URLS"])

    # dev lists the assistant since T-3181, so its catalogue carries the widget: the one general
    # assistant on every page, the home page, every organization's dataset and resource pages
    # included, and no city's (T-4342); without the component it is told nothing.
    default, by_organization = widget(rendered("dev"))
    assert default == "https://assistant.dev.joinedcontext.com/d/data/widget"
    assert by_organization == {}

    def without_assistant(tree):
        path = tree / "deployment/environments/dev/global.yaml.gotmpl"
        text = path.read_text()
        assert "\n  - assistant\n" in text
        path.write_text(text.replace("\n  - assistant\n", "\n", 1))

    assert widget(rendered_variant("dev", without_assistant)) == ("", {})


# The usage texts `jcctl publish ckan` writes on the API resources (T-3886), for a restricted
# Endpoint on dev.example: a paragraph, an indented command, a paragraph.
WS_USAGE = (
    "Live updates: open one WebSocket, subscribe to the entities you want.\n\n"
    "    websocat -H \"Authorization: Bearer $TOKEN\" 'wss://dev.example/api/endpoint/hkiair/ws'\n"
    "    {\"metadata\":{\"x-op\":\"x-subscribe\",\"x-id\":\"s1\"},\"body\":{\"type\":\"Subscription\"}}\n\n"
    "Always live: the catalogue holds this link, never a copy. Answers what the endpoint's policy lets "
    "the caller read: send a bearer token for the endpoint; without one it answers 401."
)

EVERY_SURFACE = PUBLISHED_RESOURCES + [
    {"id": "r-ws", "name": "Live updates over WebSocket", "format": "WebSocket", "url": ENDPOINT + "ws",
     "description": WS_USAGE},
    {"id": "r-ogc", "name": "OGC API - Features", "format": "OGCFEAT", "url": ENDPOINT + "ogc/features/"},
    {"id": "r-odata", "name": "OData v4", "format": "OData", "url": ENDPOINT + "odata/v4/"},
    {"id": "r-xlsx", "name": "Excel spreadsheet", "format": "XLSX", "url": ENDPOINT + "file.xlsx"},
]


def test_every_live_surface_is_in_the_api_and_live_access_section(theme):
    """T-3887, EP-108: WebSocket, OGC API - Features and OData are APIs a program calls, beside
    NGSI-LD and MCP; the spreadsheet is a download."""
    sections = {s["key"]: s for s in theme.jc_resource_sections({"resources": EVERY_SURFACE})}
    assert [i["resource"]["id"] for i in sections["api"]["items"]] == ["r-ngsi", "r-ws", "r-mcp", "r-ogc", "r-odata"]
    assert sections["api"]["title"] == "API and live access"
    assert "r-xlsx" in [i["resource"]["id"] for i in sections["data"]["items"]]
    words = {i["resource"]["id"]: i["what"] for i in sections["api"]["items"]}
    assert "WebSocket" in words["r-ws"] and "Power BI" in words["r-odata"]
    # A dataset without the WebSocket surface shows no WebSocket row.
    without = [r for r in EVERY_SURFACE if r["format"] != "WebSocket"]
    api = [s for s in theme.jc_resource_sections({"resources": without}) if s["key"] == "api"][0]
    assert all(i["format"] != "WebSocket" for i in api["items"])


def test_an_api_resource_shows_its_usage_with_its_commands_to_copy(theme):
    (ws,) = [r for r in EVERY_SURFACE if r["id"] == "r-ws"]
    blocks = theme.jc_usage(ws)
    assert [b["code"] for b in blocks] == [False, True, False]
    command = blocks[1]
    assert command["text"].startswith("websocat -H \"Authorization: Bearer $TOKEN\" 'wss://dev.example/")
    assert command["id"] == "jc-use-r-ws-1"
    # A file has no usage blocks; its description is CKAN's one-line extract.
    assert theme.jc_usage({"id": "r-csv", "format": "CSV", "url": ENDPOINT + "file.csv", "description": WS_USAGE}) == []
    assert theme.jc_usage({"format": "MCP"}) == []


@pytest.mark.parametrize("leak", [
    "    curl -H \"Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln\" 'https://dev.example/x'",
    "    curl -H \"Authorization: Bearer 0123456789abcdef0123\" 'https://dev.example/x'",
    "    websocat 'wss://context-gateway.jc.svc.cluster.local:8080/api/endpoint/hkiair/ws'",
    "Open https://dev.example.evil/x to start.",
])
def test_a_usage_block_with_a_token_or_an_internal_address_never_reaches_the_page(theme, leak):
    """EP-110: the page shows `$TOKEN` and the platform's own addresses, nothing else."""
    resource = {"id": "r", "format": "NGSI-LD", "url": ENDPOINT, "description": "Query it.\n\n" + leak}
    assert [b["text"] for b in theme.jc_usage(resource)] == ["Query it."]


def test_the_websocket_guide_is_linked_only_from_the_installation_s_documentation(theme, monkeypatch):
    ws = {"format": "WebSocket"}
    assert theme.jc_guide(ws) is None
    monkeypatch.setitem(theme.BRANDING, "documentationBaseUrl", "https://docs.dev.example/")
    assert theme.jc_guide(ws) == "https://docs.dev.example/API/07-websocket"
    assert theme.jc_guide({"format": "NGSI-LD"}) is None
    # A usage text may link the guide's host.
    linked = {"id": "r", "format": "WebSocket", "description": "Guide: https://docs.dev.example/API/07-websocket"}
    assert len(theme.jc_usage(linked)) == 1
    for unsafe in ("javascript:alert(1)", "http://docs.dev.example", "https://docs.dev.example/\"onload=x"):
        monkeypatch.setitem(theme.BRANDING, "documentationBaseUrl", unsafe)
        assert theme.jc_guide(ws) is None, unsafe


def test_an_api_resource_page_shows_its_usage_where_ckan_said_no_view():
    page = (THEME / "templates__package__resource_read.html").read_text()
    assert "(not api and h.check_access('resource_view_create'" in page
    # Its copy buttons work by base.html's jc-copy.js, loaded once for every page (T-4762).
    assert "package/snippets/jc_usage.html" in page and "/jc-copy.js" in (THEME / "templates__base.html").read_text()
    item = (THEME / "templates__package__snippets__resource_item.html").read_text()
    assert "h.jc_usage(res) if jc_section == 'api' and not url_is_edit" in item
    snippet = (THEME / "templates__package__snippets__jc_usage.html").read_text()
    assert 'data-jc-copy="{{ block.id }}"' in snippet and '<pre id="{{ block.id }}"' in snippet
    assert "|safe" not in snippet
    # The copy button reads a block's text as well as a field's value.
    assert "field.value !== undefined ? field.value : field.textContent" in (THEME / "public__jc-copy.js").read_text()


@pytest.mark.parametrize("lang, counts", [
    ("en", {1: "1 row", 2: "2 rows", 5: "5 rows", 0: "0 rows", 1234: "1 234 rows"}),
    ("sk", {1: "1 riadok", 3: "3 riadky", 5: "5 riadkov"}),
    ("cs_CZ", {1: "1 řádek", 4: "4 řádky", 12: "12 řádků"}),
    ("de", {1: "1 Zeile", 2: "2 Zeilen"}),
    # Hungarian keeps the noun singular after a number.
    ("hu", {1: "1 sor", 3: "3 sor", 5: "5 sor"}),
    ("fi", {1: "1 row", 7: "7 rows"}),
])
def test_a_row_count_takes_its_language_s_plural(theme, lang, counts):
    """T-4066: the owner's photo read "GtfsAgency 1 rows"."""
    theme.page.lang = lang
    for count, words in counts.items():
        assert theme.jc_rows(count) == words
    assert theme.jc_rows(None) == ""
    read = (THEME / "templates__package__read.html").read_text()
    assert "h.jc_t('rows')" not in read and read.count("h.jc_rows(") == 2


def test_usage_reads_as_markdown_and_its_commands_wrap(theme):
    """T-4066: `query_entities` showed its backticks, and a long command was a one-line box
    clipped mid-word behind a horizontal scrollbar."""
    snippet = (THEME / "templates__package__snippets__jc_usage.html").read_text()
    # CKAN's render_markdown runs the sanitiser; the text never reaches the page as raw HTML.
    assert "h.render_markdown(block.text)" in snippet and "<p>{{ block.text }}</p>" not in snippet
    assert "|safe" not in snippet
    css = (THEME / "public__jc-theme.css").read_text()
    # Owner, 2026-10-12: a textarea sized to its text only in Chromium; a block wraps everywhere.
    box = re.search(r"^\.jc-copy-code pre \{([^}]*)\}", css, re.M).group(1)
    assert "white-space: pre-wrap" in box and "overflow-wrap: anywhere" in box and "white-space: pre;" not in box
    assert "<textarea" not in snippet
    config = '    {"mcpServers":{"helsinki-transport":{"type":"http","url":"https://dev.example/api/endpoint/ajaezssvx3bwqgejgepmq4ig3frgan4e/mcp"}}}'
    resource = {"id": "r", "format": "MCP", "url": ENDPOINT + "mcp", "description": "Tools: `get_entity`.\n\n" + config}
    text, code = theme.jc_usage(resource)
    assert text["text"] == "Tools: `get_entity`." and not text["code"]
    assert code["code"] and code["text"] == config[4:]


def test_the_format_badge_sits_beside_the_resource_name():
    """T-4066: the badge stood alone in a wide left column beside a long usage text."""
    item = (THEME / "templates__package__snippets__resource_item.html").read_text()
    head = item[item.index('<div class="jc-resource-head">'):item.index("</div>", item.index('<div class="jc-resource-head">'))]
    assert "jc-chip" in head and 'class="heading"' in head
    css = (THEME / "public__jc-theme.css").read_text()
    assert "grid-template-columns: 6.5rem" not in css


def test_a_publisher_without_a_picture_gets_its_badge_and_words_in_the_page_language(theme, tmp_path):
    import base64
    block = tmp_path / "organizations.json"
    block.write_text(json.dumps({
        "hel-fi": {"initials": "HEL", "colour": "#0000bf", "description": {"en": "Helsinki data", "sk": "Údaje Helsínk"}},
        "bad": {"initials": "<b>", "colour": "red"},
        "evil": {"initials": "<script>", "colour": "#ffffff"},
    }))
    theme.ORGANIZATIONS_FILE = str(block)
    theme.ORGANIZATIONS = theme._organizations()
    card = theme.jc_org({"name": "hel-fi", "description": "", "image_display_url": ""})
    assert card["description"] == "Helsinki data"
    svg = base64.b64decode(card["image"].removeprefix("data:image/svg+xml;base64,")).decode()
    assert ">HEL</text>" in svg and 'fill="#0000bf"' in svg and 'fill="#ffffff"' in svg
    theme.page.lang = "sk"
    assert theme.jc_org({"name": "hel-fi"})["description"] == "Údaje Helsínk"
    # CKAN's own picture and words win; an unknown publisher keeps CKAN's placeholder.
    assert theme.jc_org({"name": "hel-fi", "description": "own", "image_display_url": "/x.png"}) == \
        {"image": "/x.png", "description": "own"}
    assert theme.jc_org({"name": "nobody"}) == {"image": None, "description": None}
    assert theme.jc_org(None) == {"image": None, "description": None}
    # A colour that is not #rgb/#rrggbb draws nothing; initials are text, never markup.
    assert theme.jc_org({"name": "bad"})["image"] is None
    evil = base64.b64decode(theme.jc_org({"name": "evil"})["image"].split(",", 1)[1]).decode()
    assert "<script>" not in evil and "&lt;scr" in evil and 'fill="#000000"' in evil


def test_a_missing_or_broken_organizations_file_leaves_ckan_s_placeholder(theme, tmp_path):
    theme.ORGANIZATIONS_FILE = str(tmp_path / "absent.json")
    assert theme._organizations() == {}
    broken = tmp_path / "broken.json"
    broken.write_text("[1, 2")
    theme.ORGANIZATIONS_FILE = str(broken)
    assert theme._organizations() == {}
    broken.write_text("[1, 2]")
    assert theme._organizations() == {}


def test_every_demo_publisher_has_a_badge_and_the_file_reaches_the_pod(rendered):
    docs = rendered("dev")
    (config,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].endswith("ckan-theme")]
    organizations = json.loads(config["data"]["organizations.json"])
    assert {"hel-fi", "praha", "zilina", "banskabystrica", "bbsk"} <= set(organizations)
    for name, entry in organizations.items():
        assert re.fullmatch(r"#[0-9a-fA-F]{6}", entry["colour"]) and entry["initials"], name
    for name in ("hel-fi", "praha", "zilina"):
        assert organizations[name]["description"]["en"] and organizations[name]["description"]["sk"], name
    (ckan,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("ckan")
               and any(c["name"] == "ckan" for c in d["spec"]["template"]["spec"]["containers"])]
    (branding,) = [v for v in ckan["spec"]["template"]["spec"]["volumes"] if v["name"] == "branding"]
    assert {"key": "organizations.json", "path": "organizations.json"} in branding["configMap"]["items"]


def test_a_dataset_page_puts_its_publisher_above_and_gives_the_data_the_whole_width():
    """Owner, 2026-10-12: the publisher sidebar took a quarter of the page from the tables."""
    base = (THEME / "templates__package__read_base.html").read_text()
    assert "{% block secondary %}{% endblock %}" in base
    assert '<div class="primary col-12" role="main">' in base and "self.primary_content()" in base
    assert "package/snippets/jc_publisher.html" in base.split("{% block pre_primary %}", 1)[1]
    bar = (THEME / "templates__package__snippets__jc_publisher.html").read_text()
    assert "h.jc_org(organization)" in bar and "placeholder-organization.png" in bar
    assert ".jc-publisher {" in (THEME / "public__jc-theme.css").read_text()


# T-4217 (photo audit T-4161): "Topics" in the main navigation opened "There are currently no
# groups for this site." and every dataset carried an empty Groups tab. Both are offered only
# once a topic exists.
def test_topics_are_offered_only_when_one_exists(theme):
    theme.page.actions["group_list"] = lambda data: []
    assert theme.jc_has_topics() is False
    theme.page.actions["group_list"] = lambda data: ["environment"]
    assert theme.jc_has_topics() is True


def test_a_topic_list_the_visitor_may_not_read_offers_no_topics(theme):
    theme.page.actions["group_list"] = theme.toolkit.NotAuthorized()
    assert theme.jc_has_topics() is False


def test_the_navigation_and_the_dataset_tabs_ask_before_offering_topics():
    header = (THEME / "header.html").read_text()
    assert "h.jc_has_topics()" in header
    tabs = (THEME / "templates__package__read_base.html").read_text()
    assert "block content_primary_nav" in tabs and "h.jc_has_topics()" in tabs


# T-4218 (photo audit T-4161): Solr split a hyphenated word into parts it OR'ed, so "bike-sharing"
# found every dataset about bikes or about sharing, and a word nobody wrote found them all
# (measured on dev: 6 against 1 as a phrase, 132 of 132 against 0). Each such word is searched
# as the phrase it is; a query with its own quotes or fields is the visitor's syntax and stays.
@pytest.mark.parametrize("asked, searched", [
    ("bike-sharing", '"bike-sharing"'),
    ("kvalita-ovzdušia Žilina", '"kvalita-ovzdušia" (Žilina OR Žilin*)'),
    ("qzxwv-nothing-is-called-this", '"qzxwv-nothing-is-called-this"'),
    ("air quality", "air (quality OR qualit*)"),
    ("-draft", "-draft"),
    ('"bike-sharing" x-y', '"bike-sharing" x-y'),
    ("tags:air-quality", "tags:air-quality"),
    ("", ""),
])
def test_a_hyphenated_word_is_searched_as_written(theme, asked, searched):
    assert theme.jc_search_words(asked) == searched


def test_the_dataset_search_takes_the_hyphenated_word_as_a_phrase(theme):
    plugin = theme.JcThemePlugin()
    assert plugin.before_dataset_search({"q": "bike-sharing", "rows": 20}) == {"q": '"bike-sharing"', "rows": 20}
    assert plugin.before_dataset_search({"fq": "x"}) == {"fq": "x"}


# T-4239 (photo audit T-4161 #3): Solr stems with the English Porter stemmer, so "ovzdušie" (air)
# found only Praha's "ovzduší" and missed the three Slovak "ovzdušia" datasets. A word with a
# Slovak or Czech letter is searched as itself or its stem with any ending, through edismax (the
# only parser of CKAN's two that takes a prefix); measured on the ckan-solr 2.11 image: dismax
# "ovzdušie" finds praha-air only, edismax "(ovzdušie OR ovzduš*)" finds praha-air and bb-air.
@pytest.mark.parametrize("asked, searched", [
    ("ovzdušie", "(ovzdušie OR ovzduš*)"),
    ("kvalita ovzdušia", "(kvalita OR kvalit*) (ovzdušia OR ovzduš*)"),
    ("Kvalita ovzduší", "(Kvalita OR Kvalit*) (ovzduší OR ovzduš*)"),
    ("školy", "(školy OR škol*)"),
    ("ľad", "ľad"),
    ("ovzduš", "(ovzduš OR ovzduš*)"),
    ('"ovzdušie"', '"ovzdušie"'),
])
def test_a_slovak_or_czech_word_finds_its_other_endings(theme, asked, searched):
    assert theme.jc_search_words(asked) == searched


# T-4387: a visitor types Slovak without its letters and in the singular; Solr folds the letters
# on both sides (CKAN's `text` field ends in ASCIIFoldingFilter), so the folded word and its stem
# reach the same Datasets. Each pair below asks for one stem: dev counted ovzdušie 4, ovzdusie 3,
# mosty 1, most 0, cesty 1, cesta 0 before.
@pytest.mark.parametrize("one, other, stem", [
    ("ovzdušie", "ovzdusie", None),
    ("most", "mosty", "most*"),
    ("cesta", "cesty", "cest*"),
    ("ovzdušie", "ovzdušia", "ovzduš*"),
])
def test_the_folded_word_and_the_singular_ask_for_the_plurals_stem(theme, one, other, stem):
    def asked(word):
        searched = theme.jc_search_words(word)
        assert searched.startswith("(%s OR " % word), searched
        return searched.split(" OR ")[1].rstrip(")")
    if stem is None:
        assert asked(one).replace("š", "s") == asked(other)
    else:
        assert asked(one) == asked(other) == stem


# T-5116: Slovak drops the vowel of -ok, -ek, -ec and -eň when the word declines, so the word's own
# prefix missed "dôchodky", "lekárne" and "Lučenca"; a short word ("deň") keeps its own prefix.
@pytest.mark.parametrize("asked, searched", [
    ("dôchodok", "(dôchodok OR dôchodok* OR dôchodk*)"),
    ("lekáreň", "(lekáreň OR lekáreň* OR lekárn*)"),
    ("Lučenec", "(Lučenec OR Lučenec* OR Lučenc*)"),
    ("výsledok", "(výsledok OR výsledok* OR výsledk*)"),
    ("deň", "deň"),
    ("potok", "(potok OR potok*)"),
])
def test_a_fleeting_vowel_word_asks_for_its_declined_stem(theme, asked, searched):
    assert theme.jc_search_words(asked) == searched


@pytest.mark.parametrize("word", ["air", "bus", "ľad", "x2", "2024", "-draft", "co2"])
def test_a_short_or_not_alphabetic_word_is_searched_as_written(theme, word):
    assert theme.jc_search_words(word) == word


def test_a_stemmed_search_asks_for_the_parser_that_takes_a_prefix(theme):
    plugin = theme.JcThemePlugin()
    assert plugin.before_dataset_search({"q": "ovzdušie"}) == {"q": "(ovzdušie OR ovzduš*)", "defType": "edismax"}
    assert plugin.before_dataset_search({"q": "air"}) == {"q": "air"}
    assert plugin.before_dataset_search({"q": "ovzdušie", "defType": "lucene"})["defType"] == "lucene"


# T-4220 (photo audit T-4161): one concept, three names. The navigation said Publishers, the
# breadcrumb, title and search box Organizations, and each card counted its CKAN members.
@pytest.mark.parametrize("entity, purpose, en, sk", [
    ("organization", "breadcrumb", "Publishers", "Vydavatelia"),
    ("organization", "page title", "Publishers", "Vydavatelia"),
    ("organization", "main nav", "Publishers", "Vydavatelia"),
    ("organization", "search placeholder", "Search publishers…", "Hľadať vydavateľov…"),
    ("group", "breadcrumb", "Topics", "Témy"),
    ("group", "search placeholder", "Search topics…", "Hľadať témy…"),
])
def test_publishers_and_topics_have_one_name(theme, entity, purpose, en, sk):
    stock = lambda *args: None
    assert theme.humanize_entity_type(stock, entity, entity, purpose) == en
    theme.page.lang = "sk"
    assert theme.humanize_entity_type(stock, entity, entity, purpose) == sk


def test_other_words_stay_ckans(theme):
    stock = lambda entity, object_type, purpose: "stock %s" % purpose
    assert theme.humanize_entity_type(stock, "organization", "organization", "save label") == "stock save label"
    assert theme.humanize_entity_type(stock, "package", "dataset", "breadcrumb") == "stock breadcrumb"


def test_a_public_publisher_card_counts_no_members():
    card = (THEME / "templates__organization__snippets__organization_item.html").read_text()
    assert "{% block members %}{% endblock %}" in card


def test_symbol_font_bullets_become_real_ones_on_the_page_and_the_api(theme):
    """T-4283: " Druh " drew as "□ Druh □" on data.dev's dataset list."""
    plugin = theme.JcThemePlugin()
    pkg = {
        "title": "Dotácie",
        "notes": "atribúty:  Rok  Typ Druh",
        "resources": [{"name": " XML", "description": None}, {"url": "x"}],
    }
    plugin.after_dataset_show({}, pkg)
    assert pkg["notes"] == "atribúty: • Rok ▪ Typ Druh"
    assert pkg["resources"][0] == {"name": "✓ XML", "description": None}
    found = plugin.after_dataset_search({"results": [{"notes": " a"}], "count": 1}, {})
    assert found == {"results": [{"notes": "• a"}], "count": 1}
    assert plugin.after_dataset_search({"count": 0}, {}) == {"count": 0}


def test_no_private_use_glyph_survives_any_text(theme):
    text = "".join(chr(c) for c in range(0xE000, 0xF900, 7))
    assert not re.search("[-�]", theme.jc_plain_glyphs(text) or "")
    assert theme.jc_plain_glyphs("Žilina ľ ô") == "Žilina ľ ô"


@pytest.mark.parametrize("lang", ["en", "sk"])
def test_the_connector_privacy_terms_and_support_pages_read_in_slovak_and_english(theme, tmp_path, lang):
    """T-4315: the pages a directory reviewer reads, from the branding block alone, no secret."""
    branding = tmp_path / "branding.json"
    branding.write_text(json.dumps({"domain": "dev.example", "contactEmail": "hello@dev.example"}))
    theme.BRANDING = theme._load()
    theme.page.lang = lang
    for name in ("connector", "privacy", "terms", "support", "accessibility"):
        page = theme.jc_page(name)
        assert page["title"] == theme.PAGES[name][lang]["title"] and page["sections"], name
        assert page["email"] == "hello@dev.example" and page["pages"] == ["connector", "privacy", "terms", "support", "accessibility"]
        text = " ".join(p for s in page["sections"] for p in s["paragraphs"])
        assert "{" not in text and not re.search(r"(?i)bearer|token|api[_-]?key|password", text), name
    assert "https://api.dev.example/api/mcp" in " ".join(theme.jc_page("connector")["sections"][0]["paragraphs"])
    assert theme.jc_page("privacy")["title"] != theme.PAGES["privacy"]["sk" if lang == "en" else "en"]["title"]
    assert theme.jc_page("imprint") is None


def test_a_page_in_another_language_reads_english_and_no_domain_drops_the_address(theme, tmp_path):
    theme.page.lang = "de"
    assert theme.jc_page("terms")["title"] == "Terms of use"
    branding = tmp_path / "branding.json"
    branding.write_text(json.dumps({"instanceName": "x", "contactEmail": "not-an-address"}))
    theme.BRANDING = theme._load()
    page = theme.jc_page("privacy")
    text = " ".join(p for s in page["sections"] for p in s["paragraphs"])
    assert page["mcp"] is None and page["email"] == "" and "{url}" not in text and "None" not in text
    assert [s["heading"] for s in theme.jc_page("terms")["sections"]] == ["Licences", "Read only", "Fair use"]


def test_the_four_pages_are_routes_of_one_template_and_every_page_links_them(theme, monkeypatch):
    rules = {}

    class Blueprint:
        def __init__(self, name, module):
            self.name = name

        def add_url_rule(self, path, endpoint, view):
            rules[path] = (endpoint, view)

        def after_app_request(self, hook):
            rules["after"] = hook

    monkeypatch.setitem(sys.modules, "flask", types.SimpleNamespace(Blueprint=Blueprint))
    theme.toolkit.render = lambda template, extra: (template, extra)
    blueprint = theme.JcThemePlugin().get_blueprint()
    # Every answer of the app passes the CSP hook (T-5147), not only this blueprint's.
    assert rules.pop("after") is theme.jc_csp_header
    assert blueprint.name == "jc_pages" and sorted(rules) == ["/accessibility", "/assistants", "/assistants/<id>", "/connector", "/map/<id>", "/map/<id>/<resource_id>", "/privacy", "/robots.txt", "/sitemap.xml", "/support", "/terms"]
    assert rules["/sitemap.xml"] == ("sitemap", theme.jc_sitemap_page) and rules["/robots.txt"] == ("robots", theme.jc_robots_page)
    monkeypatch.setattr(theme.toolkit, "config", {"ckan.site_url": "https://opendata.dev.example"}, raising=False)
    assert theme.jc_robots_page()[1:] == (200, {"Content-Type": "text/plain; charset=utf-8"})
    assert rules["/assistants"][0] == "assistants" and rules["/assistants"][1]() == ("jc/assistants.html", {})
    assert rules["/assistants/<id>"] == ("guide", theme.jc_guide_page)
    endpoint, view = rules["/privacy"]
    assert endpoint == "privacy" and view() == ("jc/page.html", {"jc_name": "privacy"})
    template = (THEME / "templates__jc__page.html").read_text()
    assert "h.jc_page(jc_name)" in template and 'snippet "snippets/jc_mcp_install.html"' in template
    footer = (THEME / "footer.html").read_text()
    assert "('connector', 'privacy', 'terms', 'support', 'accessibility')" in footer and "h.url_for('jc_pages.' ~ name)" in footer

# T-4333: a resource page reads what an Endpoint's address is from its path, says it in the page's
# language, shows an API's live rows and how to use it, and offers a file its one Download.
OD = "https://dev.example/api/endpoint/4ysiqy53qik5qyvawwaj7wgylpt4uoxa/"
OD_QUERY = "?type=StatisticalObservation&q=isPartOf%3D%3D%22https%3A%2F%2Fdata.dev.example%2Fdataset%2Fbb%22"
OD_PKG = {"name": "bb", "title": "Prírastky a úbytky občanov", "resources": [
    {"id": "n", "name": "NGSI-LD API (StatisticalObservation)", "format": "JSON-LD", "resource_type": "api",
     "url": OD + "ngsi-ld/v1/entities" + OD_QUERY, "conforms_to": "https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/",
     "description": "These rows, through the platform's NGSI-LD API (NGSI-LD data, read-only)."},
    {"id": "c", "name": "CSV (StatisticalObservation)", "format": "CSV", "resource_type": "api", "url": OD + "file.csv" + OD_QUERY},
    {"id": "m", "name": "Model Context Protocol (StatisticalObservation)", "format": "JSON", "resource_type": "api", "url": OD + "mcp"},
    {"id": "w", "name": "WebSocket (StatisticalObservation)", "format": "JSON", "resource_type": "api",
     "url": "wss://dev.example/api/endpoint/4ysiqy53qik5qyvawwaj7wgylpt4uoxa/ws"},
    {"id": "o", "name": "OData v4 (StatisticalObservation)", "format": "JSON", "resource_type": "api",
     "url": OD + "odata/v4/StatisticalObservation"},
    {"id": "t", "name": "Prírastky a úbytky občanov (tabuľka)", "format": "CSV", "datastore_active": True,
     "url": "https://data.dev.example/datastore/dump/t"},
    {"id": "p", "name": "Prírastky a úbytky občanov", "format": "JSON",
     "url": "https://egov.banskabystrica.sk/Default.aspx?NavigationState=890"},
]}


def _od(theme, rid):
    return next(r for r in OD_PKG["resources"] if r["id"] == rid)


def test_an_endpoint_address_is_read_by_its_path_not_its_format_label(theme):
    kinds = {r["id"]: theme.jc_kind(r) for r in OD_PKG["resources"]}
    assert kinds == {"n": "NGSI-LD", "c": "CSV", "m": "MCP", "w": "WEBSOCKET", "o": "ODATA", "t": None, "p": None}
    assert theme.jc_is_api(_od(theme, "n")) and theme.jc_is_api(_od(theme, "m"))
    # A file the Endpoint renders is a file, whatever resource_type the harvest wrote.
    assert not theme.jc_is_api(_od(theme, "c"))
    assert theme.jc_format(_od(theme, "n")) == "NGSI-LD" and theme.jc_format(_od(theme, "w")) == "WebSocket"
    assert theme.jc_kind({"url": "https://dev.example/api/endpoint/x/schema/v1/model.schema.json"}) == "MODEL"
    # The Endpoint's own DCAT-AP record (EP-143) is a door of the API, not a file.
    assert theme.jc_kind({"url": "https://dev.example/api/endpoint/x/"}) == "DCAT"
    assert theme.jc_kind({"url": "javascript:alert(1)"}) is None and theme.jc_kind(None) is None


def test_the_resource_list_groups_and_names_the_type_once(theme):
    sections = {s["key"]: s for s in theme.jc_resource_sections(OD_PKG)}
    assert [i["resource"]["id"] for i in sections["api"]["items"]] == ["n", "w", "m", "o"]
    assert {i["resource"]["id"] for i in sections["data"]["items"]} == {"c", "t"}
    # The harvest's link to the publisher's page is opened at the publisher, not a twin download.
    assert [i["resource"]["id"] for i in sections["source"]["items"]] == ["p"]
    labels = {r["id"]: theme.jc_resource_label(r, OD_PKG) for r in OD_PKG["resources"]}
    assert not any("(StatisticalObservation)" in label for label in labels.values()), labels
    assert labels["n"] == "NGSI-LD API" and labels["m"] == "Model Context Protocol"
    # The publisher's own page, named after the dataset, no longer reads as a twin of the table.
    assert labels["p"] == "On the publisher's site" and labels["t"] == "The publisher's original table"
    # A file or a socket is named by what is in it; its chip already names the format.
    assert labels["c"] == "All records as a table" and labels["w"] == "Live changes"
    # Each of the platform's files says what it is for, so none reads as a twin of another.
    files = [dict(_od(theme, "c"), url=OD + "file.%s" % f, format=f) for f in ("csv", "geojson", "json", "xlsx", "parquet")]
    named = [theme.jc_resource_label(r, OD_PKG) for r in files]
    assert [theme.jc_kind(r) for r in files] == ["CSV", "GEOJSON", "JSON", "XLSX", "PARQUET"]
    assert len(set(named)) == len(named)
    assert theme.jc_entity_type(_od(theme, "m")) == theme.jc_entity_type(_od(theme, "o")) == "StatisticalObservation"
    theme.page.lang = "sk"
    model = {"name": "Data model: LinkML", "format": "LinkML", "url": OD + "schema/v1/model.linkml.yaml"}
    assert theme.jc_resource_label(model, OD_PKG) == "LinkML"  # the group already says "Dátový model"
    sidebar = (THEME / "templates__package__snippets__resources.html").read_text()
    assert "h.jc_resource_sections(pkg)" in sidebar and "section.title" in sidebar


def test_the_slovak_page_carries_no_english_description(theme):
    theme.page.lang = "sk"
    what = theme.jc_what(_od(theme, "n"))
    assert what == "Živé entity cez API. Iba záznamy tohto datasetu."
    assert "Všetky záznamy typu StatisticalObservation" in theme.jc_what(_od(theme, "o"))
    # OGC API Features and OData keep the dataset's rows in their own syntax (T-4289).
    mine = "https%3A%2F%2Fdata.dev.example%2Fdataset%2Fbb"
    for url in (OD + "odata/v4/StatisticalObservation?%24filter=isPartOf%20eq%20%27" + mine + "%27",
                OD + "ogc/features/collections/StatisticalObservation/items?isPartOf=" + mine,
                # T-5205: SensorThings names it through the Datastream's Thing, after its type.
                OD + "sta/v1.1/Datastreams?%24filter=Thing%2Fproperties%2Ftype%20eq%20%27StatisticalObservation"
                   "%27%20and%20Thing%2Fproperties%2FisPartOf%20eq%20%27" + mine + "%27"):
        assert theme.jc_what(dict(_od(theme, "o"), url=url)).endswith(". Iba záznamy tohto datasetu."), url
    page = (THEME / "templates__package__resource_read.html").read_text()
    # The harvest's description reaches the page only for a resource that is not an Endpoint's.
    assert "{% if res.description and not kind and not api %}" in page
    row = (THEME / "templates__package__snippets__resource_item.html").read_text()
    assert "{% elif res.description and not kind %}" in row
    for key, words in theme.STRINGS.items():
        assert words.get("sk"), key


def test_an_address_reads_short_and_copies_whole(theme):
    address = theme.jc_address(_od(theme, "n"))
    assert address["short"] == "dev.example/api/endpoint/4ysiqy\u2026/ngsi-ld/v1/entities"
    assert address["full"] == _od(theme, "n")["url"]
    assert theme.jc_address({"url": "https://elsewhere.example/x?y=1"})["short"] == "https://elsewhere.example/x?y=1"
    # It breaks only after a slash: each segment is kept whole, "ngsi-ld" included (T-4333).
    assert address["segments"] == ["dev.example", "api", "endpoint", "4ysiqy\u2026", "ngsi-ld", "v1", "entities"]
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".jc-address-short > span { white-space: nowrap; }" in css
    page = (THEME / "templates__package__resource_read.html").read_text()
    assert 'data-jc-copy="jc-address-full"' in page and '<pre id="jc-address-full"' in page


def test_an_api_offers_no_download_and_ckan_s_top_button_is_gone():
    page = (THEME / "templates__package__resource_read.html").read_text()
    actions = page[page.index("{% block resource_actions %}"):page.index("{% endblock %}", page.index("{% block resource_actions %}"))]
    assert "res.url" not in actions and "_resource.edit" in actions
    block = page[page.index("{% block resource_view_content %}"):]
    api = block[block.index("{% elif api %}"):block.index("{% else %}", block.index("{% elif api %}"))]
    assert "jc_t('download')" not in api and "h.jc_examples(res)" in api and "data-jc-preview" in api


def test_the_live_preview_reads_five_rows_anonymously_from_the_platform_only(theme):
    preview = theme.jc_preview(_od(theme, "n"))
    assert preview["kind"] == "NGSI-LD" and "limit=5" in preview["url"] and "options=keyValues" in preview["url"]
    assert "q=isPartOf%3D%3D" in preview["url"]
    assert "%24top=5" in theme.jc_preview(_od(theme, "o"))["url"] or "$top=5" in theme.jc_preview(_od(theme, "o"))["url"]
    assert theme.jc_preview(_od(theme, "m")) is None and theme.jc_preview(_od(theme, "c")) is None
    assert theme.jc_preview({"url": "https://evil.example/api/endpoint/x/ngsi-ld/v1/entities"}) is None
    assert theme.jc_preview({"url": "http://dev.example/api/endpoint/x/ngsi-ld/v1/entities"}) is None
    script = (THEME / "public__jc-preview.js").read_text()
    assert 'credentials: "omit"' in script and "innerHTML" not in script


def test_a_quote_in_a_resource_address_stays_inside_the_shell_word(theme):
    """T-5701 (F-DEP1-003): every address put into a command goes through shlex.quote."""
    import shlex
    bad = "https://dev.example/api/endpoint/a';id;'/file.csv"
    for kind, url in (("c", bad), ("n", bad), ("o", bad), ("w", bad.replace("https", "wss")), ("m", bad)):
        for example in theme.jc_examples(dict(_od(theme, kind), url=url)):
            code = (example["code"] or "").split("\n", 1)[0]
            if not code.startswith(("curl", "websocat", "claude")):
                continue
            words = shlex.split(code)
            assert any(";id;" in w for w in words) and "id" not in words, (kind, code)
    parquet = theme.jc_examples(dict(_od(theme, "c"), url=bad))[0]["code"]
    assert parquet == "curl -L -o data.csv " + shlex.quote(bad)


def test_each_kind_shows_how_to_use_it_and_a_private_one_sends_the_token(theme):
    ngsi = theme.jc_examples(_od(theme, "n"))
    assert ngsi[0]["code"].startswith("curl -H 'Accept: application/ld+json' 'https://dev.example/") and "limit=5" in ngsi[0]["code"]
    mcp = theme.jc_examples(_od(theme, "m"))
    assert [e["title"] for e in mcp] == ["Claude (claude.ai and Claude Desktop)", "ChatGPT", "Claude Code"]
    assert all(OD + "mcp" in e["code"] for e in mcp)
    ws = theme.jc_examples(_od(theme, "w"))[0]["code"]
    assert ws.startswith("websocat --protocol ngsi-ld-ws 'wss://") and '"entities":[{"type":"StatisticalObservation"}]' in ws
    private = dict(_od(theme, "n"), description="Send `Authorization: Bearer $TOKEN`.")
    assert '-H "Authorization: Bearer $TOKEN"' in theme.jc_examples(private)[0]["code"]
    assert theme.jc_examples(_od(theme, "t")) == [] and theme.jc_examples({}) == []
    parquet = theme.jc_examples(dict(_od(theme, "c"), url=OD + "file.parquet"))[0]["code"]
    assert parquet == "curl -L -o data.parquet '%sfile.parquet'" % OD
    ids = [e["id"] for e in mcp]
    assert len(set(ids)) == len(ids)


def test_the_facts_name_the_standard_and_one_date_unless_another_day(theme):
    theme.toolkit.h.render_datetime = lambda value: str(value)[:10]
    assert theme.jc_standard(_od(theme, "n")) == {"url": _od(theme, "n")["conforms_to"], "name": "ETSI NGSI-LD (GS CIM 009)"}
    assert theme.jc_standard({"conforms_to": "not a url"}) is None
    same = {"created": "2026-10-16T10:20:46", "metadata_modified": "2026-10-16T10:20:47"}
    assert theme.jc_dates(same) == [("Updated", "2026-10-16")]
    other = {"created": "2026-10-01T10:00:00", "last_modified": "2026-10-16T10:00:00"}
    assert theme.jc_dates(other) == [("Updated", "2026-10-16"), ("Created", "2026-10-01")]
    assert theme.jc_dates({}) == []
    page = (THEME / "templates__package__resource_read.html").read_text()
    info = page[page.index("{% block resource_additional_information_inner %}"):]
    for english in ("Data last updated", "Metadata last updated", "format_resource_items"):
        assert english not in info


def test_a_hidden_package_leaves_search_and_its_page_for_all_but_a_sysadmin(theme, tmp_path):
    hidden = tmp_path / "hidden.json"
    hidden.write_text(json.dumps({"bb-adresy": "accurate: 16304 entities for 16902 rows", "Bad Name;": "x"}))
    theme.HIDDEN_FILE = str(hidden)
    theme.HIDDEN = theme._hidden()
    assert theme.HIDDEN == {"bb-adresy"}, "a name CKAN would not mint is never put into a Solr query"
    toolkit = sys.modules["ckan.plugins.toolkit"]
    plugin = theme.JcThemePlugin()

    toolkit.g = types.SimpleNamespace(userobj=None)
    params = plugin.before_dataset_search({"q": "", "fq": "+dataset_type:dataset"})
    assert params["fq"] == '+dataset_type:dataset -name:("bb-adresy")'
    with pytest.raises(toolkit.ObjectNotFound):
        plugin.after_dataset_show({}, {"name": "bb-adresy", "resources": []})
    plugin.after_dataset_show({}, {"name": "bb-dotacie", "resources": []})

    toolkit.g = types.SimpleNamespace(userobj=types.SimpleNamespace(sysadmin=True))
    assert "fq" not in plugin.before_dataset_search({"q": ""}), "a sysadmin still sees every package"
    plugin.after_dataset_show({}, {"name": "bb-adresy", "resources": []})


def test_an_editor_keeps_the_hidden_packages_of_its_own_organizations(theme, tmp_path):
    """The harvests write as an editor of their organizations (F-DEP1-002): a hidden package they
    wrote must stay findable to them, or the next run creates it again and CKAN answers 409."""
    hidden = tmp_path / "hidden.json"
    hidden.write_text(json.dumps({"bb-adresy": "accurate"}))
    theme.HIDDEN_FILE = str(hidden)
    theme.HIDDEN = theme._hidden()
    toolkit = sys.modules["ckan.plugins.toolkit"]
    plugin = theme.JcThemePlugin()
    asked = []
    theme.page.actions["organization_list_for_user"] = lambda data: asked.append(data) or [
        {"id": "org-bb"}, {"id": 'x") OR name:(*'}]
    toolkit.g = types.SimpleNamespace(userobj=types.SimpleNamespace(id="u-harvest", sysadmin=False))

    params = plugin.before_dataset_search({"q": "", "fq": "organization:banskabystrica"})
    assert params["fq"] == 'organization:banskabystrica -(+name:("bb-adresy") -owner_org:("org-bb"))', \
        "only its own organizations' hidden packages, and no id that is not one goes into Solr"
    assert asked == [{"id": "u-harvest", "permission": "update_dataset"}]
    plugin.after_dataset_show({}, {"name": "bb-adresy", "owner_org": "org-bb", "resources": []})
    with pytest.raises(toolkit.ObjectNotFound):
        plugin.after_dataset_show({}, {"name": "bb-adresy", "owner_org": "org-other", "resources": []})

    theme.page.actions["organization_list_for_user"] = lambda data: []
    params = plugin.before_dataset_search({"q": ""})
    assert params["fq"] == '-name:("bb-adresy")', "a member who edits nothing sees what the public sees"
    with pytest.raises(toolkit.ObjectNotFound):
        plugin.after_dataset_show({}, {"name": "bb-adresy", "owner_org": "org-bb", "resources": []})


def test_without_a_hidden_list_nothing_is_filtered(theme, tmp_path):
    theme.HIDDEN_FILE = str(tmp_path / "missing.json")
    theme.HIDDEN = theme._hidden()
    assert theme.HIDDEN == frozenset()
    assert "fq" not in theme.JcThemePlugin().before_dataset_search({"q": ""})


def test_the_chart_mounts_the_hidden_list_beside_the_organizations():
    chart = PROJECT_ROOT / "charts/ckan/templates"
    assert "hidden.json: |\n    {{ .Values.hidden | default dict | toJson }}" in (chart / "theme.yaml").read_text()
    assert "- key: hidden.json\n                path: hidden.json" in (chart / "ckan.yaml").read_text()


def test_more_details_name_the_publisher_s_dates_and_record_in_words(theme):
    theme.page.lang = "sk"
    theme.toolkit.h.render_datetime = lambda value: "30. novembra 2023" if value.startswith("2023-11-30") else value
    assert theme.jc_extra("issued", "2023-11-30T07:59:41Z") == ("Zverejnené vydavateľom", "30. novembra 2023", None)
    assert theme.jc_extra("source_uri", "https://data.gov.sk/set/057d") == ("Pôvodný záznam", "https://data.gov.sk/set/057d", "https://data.gov.sk/set/057d")
    assert theme.jc_extra("source_note", "Measured by HSY") == ("Source note", "Measured by HSY", None)
    assert "harvest_source" not in dict(theme.jc_more({"extras": [{"key": "harvest_source", "value": "nkod-bb"}]}))


def test_a_file_follows_its_format_s_rfc_and_an_api_base_lists_its_types(theme):
    assert theme.jc_standard(_od(theme, "c"))["name"] == "CSV (RFC 4180)"
    assert theme.jc_standard({"url": "https://elsewhere.example/a.csv", "format": "CSV"}) is None
    base = {"url": OD + "ngsi-ld/v1/", "format": "NGSI-LD"}
    assert theme.jc_preview(base) == {"url": OD + "ngsi-ld/v1/types", "kind": "NGSI-LD-TYPES"}
    assert theme.jc_examples(base)[0]["code"].endswith("ngsi-ld/v1/types'")
    # OData and OGC API without an entity set or items have no rows to preview.
    assert theme.jc_preview({"url": OD + "odata/v4/"}) is None and theme.jc_preview({"url": OD + "ogc/features/"}) is None


def test_a_short_table_gets_a_short_frame_and_the_live_address_wraps():
    css = (THEME / "public__jc-theme.css").read_text()
    # `.jc-live pre` paints commands dark; the address box stays light, its text readable (T-4333).
    assert ".jc-live .jc-live-url { padding: .5rem .75rem; background: var(--jc-surface); color: var(--jc-ink);" in css
    page = (THEME / "templates__package__read.html").read_text()
    assert 'data-jc-rows="{{ [preview.total or 20, 20]|min }}"' in page
    assert '<pre id="jc-live-url"' in page and 'readonly value="{{ live.url }}"' not in page
    css = (THEME / "public__jc-theme.css").read_text()
    assert "calc(15rem + var(--jc-rows, 20) * 2.6rem)" in css


def test_the_dataset_facts_name_the_standard_and_the_region(theme):
    rows = {label: (text, link) for label, text, link in theme.jc_about({"extras": [
        {"key": "conforms_to", "value": "https://www.w3.org/TR/json-ld11/"},
        {"key": "spatial_uri", "value": "http://data.europa.eu/nuts/code/SK031"}]})}
    assert rows["Follows the standard"] == ("JSON-LD 1.1", "https://www.w3.org/TR/json-ld11/")
    assert rows["Area covered"] == ("Žilina Region (NUTS SK031)", "http://data.europa.eu/nuts/code/SK031")
    assert theme.jc_about({"extras": [{"key": "spatial_uri", "value": "http://data.europa.eu/nuts/code/FI1B1"}]})[0][1] == "NUTS FI1B1"
    assert theme.jc_extra("quality_pass_rate", "1.0")[1] == "100 %"


def test_a_command_breaks_only_between_its_pieces(theme):
    # The owner's rule: a URL never breaks inside a word, a host, an id or "ngsi-ld" (T-4333).
    pieces = theme.jc_pieces("curl -H 'Accept: x' 'https://dev.example/api/endpoint/4ysiqy/ngsi-ld/v1/entities?type=A&limit=5'")
    assert "".join(p["text"] for p in pieces).startswith("curl -H 'Accept: x' 'https://")
    words = [p["text"] for p in pieces if not p["space"]]
    assert words[4] == "'https://" and "ngsi-ld/" in words and "4ysiqy/" in words and "entities?" in words and "type=A&" in words
    assert all(not p["text"].isspace() or p["space"] for p in pieces)
    assert theme.jc_pieces("") == [] and theme.jc_pieces(None) == []
    snippet = (THEME / "templates__snippets__jc_breakable.html").read_text()
    assert not snippet.endswith("\n")  # inside <pre> a trailing newline is a blank line
    for name in ("templates__package__read.html", "templates__package__resource_read.html",
                 "templates__package__snippets__jc_usage.html", "templates__snippets__jc_mcp_install.html"):
        page = (THEME / name).read_text()
        assert "snippets/jc_breakable.html" in page and not re.search(r"<pre[^>]*><code>\{\{", page), name
    css = (THEME / "public__jc-theme.css").read_text()
    assert "pre .jc-nb { white-space: pre; }" in css and "pre:has(.jc-nb) { overflow-x: auto; }" in css


def test_the_framed_table_has_no_developer_lines_and_room_below_desktop():
    css = (THEME / "public__jc-theme.css").read_text()
    assert "body.dt-view .resourceinfo, body.dt-view .sortinfo { display: none; }" in css
    assert "body.dt-view .dataTables_filter label::after { content: none; }" in css
    assert "clamp(27rem, calc(24rem + var(--jc-rows, 20) * 2.6rem), 34rem)" in css


# T-4585: "Use it in your AI assistant", one page per installation and a button beside the chat's.
@pytest.mark.parametrize("domain, name", [("data.dev.example", "Banská Bystrica otvorené dáta"), ("hel.example", "Helsinki Open Data")])
def test_the_assistants_page_names_the_installation_s_own_hub_and_assistants_in_order(theme, tmp_path, domain, name):
    branding = tmp_path / "branding.json"
    branding.write_text(json.dumps({"domain": domain, "instanceName": name}))
    theme.BRANDING = theme._load()
    page = theme.jc_assistants()
    hub = f"https://api.{domain}/api/mcp"
    # T-4735: the lead names the connector as each guide has the person name it, not the instance.
    assert page["mcp"]["url"] == hub and "Joinedcontext Open Data" in page["lead"] and "{" not in page["lead"]
    assert name not in page["lead"]
    cards = {card["id"]: card for card in page["cards"]}
    # T-4634, T-4643: Gemini's app needs OAuth (T-4631), so it has no card until its flag is on.
    assert [card["id"] for card in page["cards"]] == ["chatgpt", "claude", "grok"]
    assert cards["chatgpt"]["guide_label"] == "Guide"
    assert "gemini" not in page  # no Gemini CLI command either (T-4735)
    for card in page["cards"]:
        # T-4735: no store link; the card is its logo, its name and its guide.
        assert set(card) == {"id", "name", "logo", "guide_label"}, card
        assert (THEME / ("templates__" + card["logo"].replace("/", "__"))).is_file(), card["logo"]
        assert "token" not in json.dumps(card).lower()


def test_the_assistants_page_is_left_empty_without_a_plain_domain(theme):
    theme.BRANDING["domain"] = "dev.example/evil"
    assert theme.jc_assistants() is None


def test_the_assistants_page_reads_in_slovak(theme):
    theme.page.lang = "sk"
    page = theme.jc_assistants()
    assert [card["guide_label"] for card in page["cards"]] == ["Návod"] * 3
    assert theme.jc_t("connector_status") == ("Konektor pripravujeme: v adresároch Claude a ChatGPT ešte nie je "
                                              "zverejnený. Zatiaľ ho pridáte ako vlastný konektor podľa návodu.")
    assert theme.jc_t("coming_soon") == "Čoskoro" and theme.jc_t("for_developers") == "Pre vývojárov"
    assert theme.jc_t("store_address") == "Každý návod ukazuje, kam vložiť túto adresu:"
    assert "add_to" not in theme.STRINGS
    # Nothing on /assistants says "Pridať do …", the editors' buttons in its fold included (T-4735).
    assert not [k for k, words in theme.STRINGS.items()
                if re.search(r"Pridať do|Add to|Přidat do|^Zu \w+ hinzufügen", " ".join(map(str, words.values())))]
    for key in ("assistants", "assistants_lead", "connector_status", "store_address", "for_developers", "coming_soon",
                "copy", "any_client", "guide_open", "guide_title", "guide_who", "guide_steps", "guide_try",
                "guide_example", "guide_good", "guide_trouble", "guide_checked", "guide_back"):
        assert set(theme.STRINGS[key]) == LANGUAGES, key
        # T-4628: the owner read English store words inside the Slovak steps.
        assert "Add custom" not in theme.STRINGS[key]["sk"] and "Create as" not in theme.STRINGS[key]["sk"], key


def test_the_assistants_page_offers_each_card_s_own_guide_and_links_nowhere_else():
    template = (THEME / "templates__jc__assistants.html").read_text()
    assert "{% set page = h.jc_assistants() %}" in template
    assert template.count("href=") == 3 and "card.open" not in template and 'target="_blank"' not in template
    # The third: each website the hub searches (T-4659), its own address.
    assert '<a href="{{ source.url }}" rel="noopener noreferrer">' in template
    # T-4735: the guide is the card's one link, and the whole card opens it.
    assert ('<a class="jc-card-link" href="{{ h.url_for(\'jc_pages.guide\', id=card.id) }}" '
            'aria-describedby="jc-card-{{ card.id }}">{{ card.guide_label }}</a>') in template
    # T-4634: no card says "Coming soon"; the page's empty state alone does.
    assert template.count("coming_soon") == 1 and "{% if card.soon %}" not in template
    assert template.count("data-jc-copy=") == 1 and 'data-jc-copy="jc-store-url"' in template
    assert '{% include "jc/connector_note.html" %}' in template
    assert "<img" not in template and "<script>" not in template


def test_the_corner_offers_the_assistants_page_beside_the_chat_in_one_row():
    footer = (THEME / "footer.html").read_text()
    corner = footer.split('<div class="jc-corner">', 1)[1].split("</div>", 1)[0]
    link = corner.index('href="{{ h.url_for(\'jc_pages.assistants\') }}"')
    assert link < corner.index('<details class="jc-assistant" id="jc-assistant">')
    assert "aria-label=\"{{ h.jc_t('assistants_label') }}\"" in corner
    # T-4609: the footer's light link colour no longer reaches it, and the open panel hides it.
    assert ".jc-corner .jc-assistants-link { box-sizing" in desktop_css() and ".jc-corner:has(.jc-assistant[open]) .jc-assistants-link { display: none; }" in desktop_css()
    assert "{% set assistants = h.jc_mcp_install() %}" in footer and "{% if widget or assistants %}" in footer
    css = (THEME / "public__jc-theme.css").read_text()
    desktop = css.split("@media (max-width: 40rem) {", 1)[0]
    assert ".jc-corner { position: fixed;" in desktop and "display: flex; align-items: flex-end; gap: .5rem; }" in desktop
    assert ".jc-assistant { position: relative; }" in desktop and "min-block-size: 2.75rem;" in desktop
    phone = css.split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    # T-4613: the logo pill fits beside the launcher, so a phone keeps one row and the desktop's room.
    assert ".jc-corner { align-items: center; }" in phone and "flex-direction: column" not in phone.split(".jc-assistant[open]")[0]
    assert "padding-block-end: 8.5rem" not in phone


def desktop_css():
    return (THEME / "public__jc-theme.css").read_text().split("@media (max-width: 40rem) {", 1)[0]


def render(template, theme, **context):
    """A theme template through Jinja, as CKAN renders it: `page.html` a stand-in that prints the
    blocks, CKAN's `{% snippet %}` tag dropped (its own template is tested above)."""
    jinja2 = pytest.importorskip("jinja2")
    sources = {path.name.replace("templates__", "", 1).replace("__", "/"): path.read_text()
               for path in THEME.glob("templates__*")}
    sources["page.html"] = "{% block primary %}{% endblock %}{% block scripts %}{% endblock %}"
    # The footer's corner alone: the rest of the footer extends CKAN's own.
    footer = (THEME / "footer.html").read_text()
    sources["corner.html"] = footer[footer.index("{% set widget"):footer.rindex("{% endblock %}", 0, footer.index("{% block footer_links_ckan"))]
    sources = {k: re.sub(r"{%\s*snippet [^%]*%}", "", v) for k, v in sources.items()}
    env = jinja2.Environment(loader=jinja2.DictLoader(sources), autoescape=True)
    h = types.SimpleNamespace(jc_assistants=theme.jc_assistants, jc_t=theme.jc_t, jc_mcp_install=theme.jc_mcp_install,
                              jc_page=theme.jc_page, lang=lambda: "en",
                              jc_assistant_widget=lambda publisher: context.get("widget"),
                              url_for=lambda name, **kw: "/assistants" + ("/" + kw["id"] if "id" in kw else ""),
                              url_for_static=lambda path: path)
    return env.get_template(template).render(h=h, **context)


@pytest.mark.parametrize("lang", ["sk", "en", "cs", "de", "hu"])
def test_each_assistant_card_is_its_logo_its_name_and_its_guide(theme, lang):
    """T-4628, T-4735, the owner: "there should be just návod/guide", no store link. T-4609
    printed `card.copy` as "<built-in method copy …>": Jinja reads a dict's method before its key."""
    theme.page.lang = lang
    html = render("jc/assistants.html", theme)
    assert "built-in method" not in html and "0x" not in html
    blocks = re.findall(r'<li class="jc-assistant-card">(.*?)</li>', html, re.S)
    cards = {re.search(r'id="jc-card-(\w+)"', block).group(1): block for block in blocks}
    assert list(cards) == ["chatgpt", "claude", "grok"]
    # T-4735: one calm line says no directory lists the connector yet, above the cards.
    assert html.count('<p class="jc-connector-status">') == 1
    assert html.index("jc-connector-status") < html.index("jc-assistant-cards")
    assert not re.search(r"Pridať do|Add to|Přidat do|Zu \w+ hinzufügen|chatgpt\.com/plugins|claude\.ai/customize|grok\.com/connectors", html)
    for id, card in cards.items():
        heading = re.search(r"<h2[^>]*>(.*?)</h2>", card, re.S).group(1)
        assert heading.startswith('<svg class="jc-ai-logo" width="32" height="32" aria-hidden="true"'), id
        assert re.sub(r"<svg.*</svg>", "", heading, flags=re.S).strip() in {"ChatGPT", "Claude", "Grok"}, id
        assert re.findall(r'<a [^>]*href="([^"]*)"[^>]*>', card) == [f"/assistants/{id}"], id
        assert 'class="jc-card-link"' in card and "target=" not in card and "Coming soon" not in card, id
        assert "jc-code" not in card and "<button" not in card and "<pre" not in card, id
    # The address once for the page, as text with one copy button; nothing technical above the fold.
    hub = "https://api.dev.example/api/mcp"
    above, fold = html.split('<details class="jc-developers">', 1)
    # It breaks only before a slash, never inside a word (T-4284), and a copy reads it whole.
    assert '<span id="jc-store-url" class="jc-store-url">https://api.dev.example<wbr>/api<wbr>/mcp</span>' in above
    assert above.replace("<wbr>", "").count(hub) == 1
    # The address's copy button and the note's example question's (T-4735).
    assert above.count("<button") == 2 and not re.search(r"class=\"jc-code[ \"]", above) and "<pre" not in above and "<code" not in above
    if lang == "en":
        assert fold.startswith("\n        <summary>For developers</summary>")


@pytest.mark.parametrize("lang, line", [
    ("sk", "Konektor pripravujeme: v adresároch Claude a ChatGPT ešte nie je zverejnený."),
    ("en", "We are still preparing this connector: it is not yet published in the Claude or ChatGPT directories.")])
def test_the_connector_page_says_once_under_its_heading_that_no_directory_lists_it_yet(theme, lang, line):
    """T-4735, the owner: the connector "is working on, yet, not published"; /privacy says nothing of it."""
    theme.page.lang = lang
    html = render("jc/page.html", theme, jc_name="connector")
    assert html.count(line) == 1 and html.index("page-heading") < html.index(line) < html.index("<h2")
    assert "jc-connector-status" not in render("jc/page.html", theme, jc_name="privacy")
    assert line in render("jc/assistants.html", theme)


NOTE = {"sk": ("Dôležité: asistent konektor väčšinou sám nepoužije.",
               "Použi Joinedcontext Open Data: Aké datasety o kvalite ovzdušia máte?",
               "Asistent spozná konektor podľa mena, ktoré ste mu dali pri pridaní.", "Vyskúšajte"),
        "en": ("Important: the assistant usually won't use the connector on its own.",
               "Use Joinedcontext Open Data: Which datasets about air quality do you have?",
               "The assistant recognises the connector by the name you gave it when you added it.", "Try it")}


@pytest.mark.parametrize("lang", sorted(NOTE))
@pytest.mark.parametrize("page", ["assistants", "chatgpt", "claude", "grok"])
def test_the_note_says_to_name_the_connector_with_an_example_to_copy(theme, lang, page):
    """T-4735, the owner: an assistant seldom uses the connector unasked; one calm note says to name
    it, by the name the guide has the person give it, with an example question and its copy button."""
    theme.page.lang = lang
    theme.BRANDING["samples"] = {}
    if page == "assistants":
        html = render("jc/assistants.html", theme)
        # Right under the status line, above the cards.
        assert html.index("jc-connector-status") < html.index("jc-connector-note") < html.index("jc-assistant-cards")
    else:
        html = render("jc/guide.html", theme, jc_guide=theme.jc_assistant_guide(page))
        # After the steps, above "Try it"; the guide's own question names the connector first too.
        assert html.index("</ol>") < html.index("jc-connector-note") < html.index(f">{NOTE[lang][3]}</h2>")
        example = re.search(r'<blockquote class="jc-guide-example">(.*?)</blockquote>', html).group(1)
        assert html_lib.unescape(example) == NOTE[lang][1], example
        # A step that names the connector types the same name; no other name is given.
        steps = " ".join(re.findall(r"<li>(.*?)</li>", html, re.S))
        assert page == "grok" or "Joinedcontext Open Data" in steps
        assert "joinedcontext<" not in html and " joinedcontext." not in html
    note = html_lib.unescape(re.search(r'<div class="jc-connector-note" role="note">(.*?)\n</div>', html, re.S).group(1))
    lead, prompt, why, _ = NOTE[lang]
    assert lead in note and why in note and 'aria-hidden="true"' in note
    assert f'<q id="jc-note-prompt">{prompt}</q>' in note
    assert 'data-jc-copy="jc-note-prompt"' in note and html.count('id="jc-note-prompt"') == 1


def test_the_note_is_calm_and_every_language_has_its_words(theme):
    css = (THEME / "public__jc-theme.css").read_text()
    rule = re.search(r"^\.jc-connector-note \{([^}]*)\}", css, re.M).group(1)
    assert "var(--jc-primary-soft)" in rule and "danger" not in rule and "red" not in rule
    for key in ("connector_note", "connector_why", "guide_use"):
        assert set(theme.STRINGS[key]) == LANGUAGES, key
    assert all("{connector}" in words for words in theme.STRINGS["guide_use"].values())
    # The instance's own question, named: dev asks about Helsinki's air (T-4636 keeps cities out of the theme).
    theme.BRANDING["samples"] = {"sk": ["Aká je teraz kvalita ovzdušia v Helsinkách?"]}
    assert theme.jc_try("sk") == "Použi Joinedcontext Open Data: Aká je teraz kvalita ovzdušia v Helsinkách?"
    assert theme.CONNECTOR_NAME == "Joinedcontext Open Data" and theme.MCP_NAME == "joinedcontext"


def test_the_developers_fold_holds_every_command_and_stays_closed():
    page = (THEME / "templates__jc__assistants.html").read_text()
    fold = page.split('<details class="jc-developers">', 1)[1].split("</details>", 1)[0]
    assert "{% snippet 'snippets/jc_mcp_install.html', editors_only=True, live=True %}" in fold
    assert re.findall(r"<details[^>]*>", page) == ['<details class="jc-developers">']
    snippet = (THEME / "templates__snippets__jc_mcp_install.html").read_text()
    editors = snippet.split("{% if not editors_only %}\n    <div>", 1)[1].split("{% endif %}", 1)[1]
    assert "gemini" not in snippet.lower()  # T-4735
    for block in ("mcp.vscode", "mcp.cursor", 'id="jc-mcp-claude"', 'id="jc-mcp-json"'):
        assert block in editors, block


def test_the_card_logos_are_the_makers_own_in_their_colours_and_safe():
    sources = {"chatgpt": "https://developers.openai.com/favicon.svg", "claude": "https://claude.ai/favicon.svg",
               "grok": "https://grok.com/images/favicon.svg",
               "gemini": "https://www.gstatic.com/lamda/images/gemini_sparkle_v002_d4735304ff6292a690345.svg"}
    colours = {"chatgpt": "#0080F7", "claude": "#D97757", "grok": "#050505", "gemini": "#1BA1E3"}
    assert sorted(p.name for p in THEME.glob("templates__jc__card-*.svg")) == [f"templates__jc__card-{id}.svg" for id in sorted(sources)]
    for id, source in sources.items():
        comment, svg = (THEME / f"templates__jc__card-{id}.svg").read_text().split("-#}\n", 1)
        assert source in comment and "fetched 2026-10-18" in comment, id
        assert svg.startswith('<svg class="jc-ai-logo" width="32" height="32" aria-hidden="true" focusable="false"'), id
        assert colours[id] in svg and 'fill="#fff"' not in svg, id
        assert not re.search(r"<script|<foreignObject|\son\w+=|href=|filter=|<style", svg, re.I), id
        # Ids are prefixed, so two marks on one page never take each other's gradient or mask.
        assert all(i.startswith(f"jc-{id}-") for i in re.findall(r'\sid="([^"]+)"', svg)), id
        assert all(u.startswith(f"#jc-{id}-") for u in re.findall(r"url\(([^)]+)\)", svg)), id
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".jc-assistant-card .jc-ai-logo { flex: none; display: block; inline-size: 2rem; block-size: 2rem; }" in css
    # The corner's slant stays on the corner's segments.
    assert ".jc-seg .jc-ai-logo { flex: none; display: block; inline-size: 1.375rem; block-size: 1.375rem; transform: skewX(14deg); }" in css
    assert not re.search(r"(^|\n)\s*\.jc-ai-logo \{", css)


def test_the_cards_sit_in_one_row_or_as_many_as_fit():
    css = (THEME / "public__jc-theme.css").read_text()
    assert "display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 14rem), 1fr)); grid-auto-rows: 1fr; gap: 1rem; }" in css
    assert ".jc-soon" not in css
    assert ".jc-card-link::after { content: \"\"; position: absolute; inset: 0; border-radius: inherit; }" in css
    assert ".jc-assistant-card:has(.jc-card-link:focus-visible) { outline: 3px solid var(--jc-accent); outline-offset: 2px; }" in css


LABELS = {"en": "Use the data in ChatGPT, Claude or any MCP client",
          "sk": "Použite dáta v ChatGPT, Claude alebo v akomkoľvek klientovi MCP",
          "cs": "Použijte data v ChatGPT, Claude nebo v libovolném klientovi MCP",
          "de": "Die Daten in ChatGPT, Claude oder jedem MCP-Client nutzen"}


@pytest.mark.parametrize("lang", sorted(LABELS))
def test_the_corner_link_is_three_logos_and_no_words(theme, lang):
    """T-4613, T-4620, T-4735: Claude, ChatGPT, MCP, each on its own segment, and no Gemini; no
    text, no "/"; the name is the aria-label, in the page's language."""
    theme.page.lang = lang
    html = render("corner.html", theme, publisher=None, widget="https://dev.example/assistant/")
    tag, link = re.search(r'(<a class="jc-assistants-link"[^>]*>)(.*?)</a>', html, re.S).groups()
    assert f'aria-label="{LABELS[lang]}"' in tag
    segments = re.findall(r'<span class="jc-seg jc-seg-(\w+)">(<svg class="jc-ai-logo".*?</svg>)</span>', link, re.S)
    assert [mark for mark, _ in segments] == ["claude", "chatgpt", "mcp"]
    assert link.count("<svg") == 3
    assert "gemini" not in html.lower()
    segments = [svg for _, svg in segments]
    assert all('aria-hidden="true"' in s for s in segments)
    # every text node of the link is blank: no word and no "/" between the logos
    assert re.sub(r"<[^>]+>", "", link).strip() == ""
    assert '<details class="jc-assistant" id="jc-assistant">' in html


def test_the_brand_logos_are_the_makers_own_saved_here_with_their_source():
    for name, source in (("claude", "https://claude.ai/"), ("chatgpt", "https://developers.openai.com/"),
                         ("mcp", "https://modelcontextprotocol.io/")):
        text = (THEME / f"templates__jc__brand-{name}.svg").read_text()
        comment, svg = text.split("\n", 1)
        assert comment.startswith("{#") and f"from {source}" in comment, name
        # served from CKAN's origin: nothing in the mark loads from anywhere
        assert not re.search(r"href=|url\(", svg), name
        # T-4620: white on the maker's colour, which the segment behind it carries
        assert set(re.findall(r'(?:fill|stroke)="([^"]+)"', svg)) <= {"#fff", "none"}, name
        assert svg.startswith('<svg class="jc-ai-logo" width="24" height="24" aria-hidden="true" focusable="false"'), name


def test_the_bar_is_one_slant_in_the_makers_colours_and_shrinks_on_a_phone():
    """T-4620: one skewed bar with its logos upright, each segment in its maker's colour."""
    css = desktop_css()
    assert "transform: skewX(-14deg);" in css and "transform: skewX(14deg);" in css
    assert "flex-wrap: nowrap;" in css and "border-radius: 10px;" in css
    for mark, ground in (("claude", "#d97757"), ("chatgpt", "#10a37f"), ("mcp", "#1f1f2e")):
        assert f".jc-seg-{mark} {{ background: {ground}; }}" in css, mark
    assert "jc-seg-gemini" not in css  # T-4735
    assert "--jc-slant" not in css and "clip-path: polygon" not in css
    phone = (THEME / "public__jc-theme.css").read_text().split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    assert ".jc-corner .jc-assistants-link { block-size: 2.75rem; }" in phone


def test_the_closed_launcher_takes_the_bar_s_shape_and_the_open_one_does_not():
    """T-4622: closed, the launcher has the bar's slant, corners, shadow and height, its words
    upright on one line; open, it is hidden (T-4635), so no skew reaches it."""
    css = desktop_css()
    bar = css.split(".jc-corner .jc-assistants-link { ", 1)[1].split("}", 1)[0]
    closed = css.split(".jc-assistant:not([open]) > summary { ", 1)[1].split("}", 1)[0]
    for shared in ("block-size: 3rem;", "border-radius: 10px;", "transform: skewX(-14deg);",
                   "margin-inline: .375rem;",
                   "box-shadow: 0 4px 14px color-mix(in oklab, var(--jc-ink) 25%, transparent);"):
        assert shared in bar and shared in closed, shared
    assert "white-space: nowrap;" in closed
    assert ".jc-assistant:not([open]) > summary > span { display: inline-block; transform: skewX(14deg); }" in css
    for rule in re.findall(r"[^{}]*\{[^}]*skewX[^}]*\}", css):
        assert "[open]" not in rule.split("{", 1)[0] or ":not([open])" in rule, rule
    assert "transition" not in closed and "animation" not in closed
    phone = (THEME / "public__jc-theme.css").read_text().split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    assert ".jc-assistant:not([open]) > summary { block-size: 2.75rem; inline-size: 2.75rem; padding-inline: 0; }" in phone
    # the closed summary's focus ring stays: no rule takes the outline away
    assert ".jc-assistant > summary:focus-visible { outline: 3px solid var(--jc-accent); outline-offset: 2px; }" in css
    assert "outline: none" not in closed and "outline: 0" not in closed


def test_the_corner_stays_inside_the_gutters_whatever_the_label():
    """T-4622 review: the German label is 278px at 15px, so at 320px no row holds it and the bar.
    On a phone the corner is at most the screen less the two 1rem gutters and wraps the bar onto
    a row above the launcher instead of pushing it off the left edge; the page keeps room for
    the second row. Measured on data.dev (Playwright, every label, 390/375/360/320): sk and cs
    keep one row down to 375, en to 360, and no box leaves [16, width - 16]. Since T-4996 the
    phone's launcher is an icon, so one row holds at 320 too."""
    phone = (THEME / "public__jc-theme.css").read_text().split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    assert ".jc-corner { flex-wrap: wrap; justify-content: flex-end; max-inline-size: calc(100vw - 2rem); }" in phone
    assert "body:has(.jc-corner) { padding-block-end: 4.5rem; }" in phone  # T-4996: one row
    assert ".jc-seg { inline-size: 1.4375rem; }" in phone
    # the launcher itself never wraps its words: the corner wraps, not the label
    assert "white-space: nowrap;" in desktop_css().split(".jc-assistant:not([open]) > summary { ", 1)[1].split("}", 1)[0]


def test_on_a_phone_the_closed_launcher_is_an_icon_that_keeps_its_name():
    """T-4996: at 400px the worded launcher covered dataset cards' chips and text. On a phone the
    closed launcher is a square chat icon of the bar's height; its words stay in the summary,
    clipped for the eye only, so a screen reader still names the button; the page's end keeps room
    for the one row the corner now needs."""
    footer = (THEME / "footer.html").read_text()
    summary = footer.split("<summary>", 1)[1].split("</summary>", 1)[0]
    assert '<svg class="jc-assistant-icon" aria-hidden="true" focusable="false"' in summary
    assert summary.endswith("<span>{{ h.jc_t('assistant') }}</span>")
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".jc-assistant-icon { display: none; }" in desktop_css()
    phone = css.split("@media (max-width: 40rem) {", 1)[1].split("\n}\n", 1)[0]
    clipped = phone.split(".jc-assistant:not([open]) > summary > span { ", 1)[1].split("}", 1)[0]
    assert "clip-path: inset(50%);" in clipped and "inline-size: 1px;" in clipped
    assert "display: none" not in clipped, "the name must stay in the accessibility tree"
    assert ".jc-assistant:not([open]) > summary > .jc-assistant-icon { display: block;" in phone
    assert "body:has(.jc-corner) { padding-block-end: 4.5rem; }" in phone


def _with_branding(theme, monkeypatch, tmp_path, block):
    """The plugin reloaded with `block` as its branding file."""
    branding = tmp_path / "instance-branding.json"
    branding.write_text(json.dumps(block))
    monkeypatch.setenv("JC_BRANDING_FILE", str(branding))
    spec = importlib.util.spec_from_file_location("jc_theme_plugin_instance", THEME / "plugin.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.page = theme.page
    return module


INSTANCE = {"domain": "dev.example", "instanceName": "joinedcontext",
            "languages": {"default": "sk", "offered": ["sk", "en", "cs"]},
            "siteTitle": {"sk": "Otvorené dáta", "en": "Open data"},
            "samples": {"en": ["How many buses run now?"], "sk": ["Koľko autobusov teraz jazdí?"]}}


def test_the_site_title_is_the_instances_in_the_readers_language(theme, monkeypatch, tmp_path):
    """T-4636: `site.title` per language, the default language for one it lacks, the
    installation's name when the instance sets none."""
    module = _with_branding(theme, monkeypatch, tmp_path, INSTANCE)
    for lang, title in (("sk", "Otvorené dáta"), ("en", "Open data"), ("cs", "Otvorené dáta")):
        theme.page.lang = lang
        assert module.jc_site_title() == title, lang
    assert module.jc_site_title("en") == "Open data"
    theme.page.lang = "en"
    assert theme.jc_site_title() == "joinedcontext"
    bare = _with_branding(theme, monkeypatch, tmp_path, dict(INSTANCE, siteTitle={"sk": "", "en": 3}))
    assert bare.jc_site_title() == "joinedcontext", "nothing usable falls back to the installation's name"


def test_the_tagline_is_the_instances_then_the_installations_then_the_themes(theme, monkeypatch, tmp_path):
    """T-4636, owner 15:18Z: `site.tagline` per language under the title, which it no longer
    repeats; the installation's tagline, then the theme's own line, where the instance sets none."""
    module = _with_branding(theme, monkeypatch, tmp_path,
                            dict(INSTANCE, siteTagline={"sk": "Dáta mesta.", "en": "The city's data."}))
    theme.page.lang = "en"
    assert module.jc_site_tagline() == "The city's data."
    theme.page.lang = "cs"
    assert module.jc_site_tagline() == "Dáta mesta.", "a language it lacks takes the default's"
    theme.page.lang = "sk"
    plain = _with_branding(theme, monkeypatch, tmp_path, dict(INSTANCE, tagline="Ours."))
    assert plain.jc_site_tagline() == "Ours."
    bare = _with_branding(theme, monkeypatch, tmp_path, INSTANCE)
    assert bare.jc_site_tagline() == "Prezrite si ich, stiahnite alebo sa na ne opýtajte asistenta."
    assert "Otvorené dáta" not in bare.jc_site_tagline(), "the line does not repeat the title"
    home = (THEME / "templates__home__index.html").read_text()
    assert "h.jc_site_tagline()" in home


def test_the_header_logo_is_the_instances_file_or_the_lockup(theme, monkeypatch, tmp_path):
    """T-4636: a file name in the theme's public directory, never a path or another origin."""
    assert theme.jc_site_logo() == "/jc-logo.svg"
    for logo, served in (("city.svg", "/city.svg"), ("example-city_2.png", "/example-city_2.png"),
                         ("../etc/passwd", "/jc-logo.svg"), ("https://evil.example/x.svg", "/jc-logo.svg"),
                         ("/abs.svg", "/jc-logo.svg"), ("x.svg?a", "/jc-logo.svg"), ("", "/jc-logo.svg")):
        module = _with_branding(theme, monkeypatch, tmp_path, dict(INSTANCE, siteLogo=logo))
        assert module.jc_site_logo() == served, logo


def test_the_connector_pages_questions_are_the_instances(theme, monkeypatch, tmp_path):
    """T-4636: "Try it" lists the instance's own questions in the reader's language; with none,
    the page has no such section and names no city."""
    module = _with_branding(theme, monkeypatch, tmp_path, INSTANCE)
    theme.page.lang = "en"
    sections = module.jc_page("connector")["sections"]
    assert sections[-1] == {"heading": "Try it", "paragraphs": ["How many buses run now?"]}
    theme.page.lang = "sk"
    assert module.jc_page("connector")["sections"][-1]["paragraphs"] == ["Koľko autobusov teraz jazdí?"]
    theme.page.lang = "cs"
    assert module.jc_page("connector")["sections"][-1]["paragraphs"] == ["Koľko autobusov teraz jazdí?"], \
        "a language without questions takes the default language's"
    assert module.jc_page("privacy")["sections"][-1]["heading"] != "Try it"
    theme.page.lang = "en"
    assert all(s["heading"] != "Try it" for s in theme.jc_page("connector")["sections"])
    odd = _with_branding(theme, monkeypatch, tmp_path, dict(INSTANCE, samples={"en": ["ok", 3, " ", None]}))
    assert odd.jc_page("connector")["sections"][-1]["paragraphs"] == ["ok"]


def test_the_nuts_names_are_a_data_file_beside_the_plugin(theme):
    """T-4636: the region names moved out of the code; the chart mounts the file beside plugin.py."""
    assert theme.NUTS_NAMES["CZ010"]["en"] == "Prague"
    chart = (PROJECT_ROOT / "charts/ckan/templates/ckan.yaml").read_text()
    assert "mountPath: /srv/theme/ckanext_jc_theme/nuts.json" in chart


def test_no_city_is_named_in_the_theme_code_or_the_chart_templates():
    """T-4636 Done when: the city names live in the instance's values, not the theme or the chart."""
    banned = re.compile(r"bansk|bbsk|žilin|zilin|praha|helsink|data\.dev", re.I)
    files = [THEME / "plugin.py", *THEME.glob("*.html"), *(PROJECT_ROOT / "charts/ckan/templates").glob("*")]
    for path in files:
        text = path.read_text()
        if path.suffix == ".html":
            text = re.sub(r"\{#.*?#\}", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S)
        if path.suffix in (".yaml", ".tpl"):
            text = re.sub(r"\{\{-?\s*/\*.*?\*/\s*-?\}\}", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S)
        for number, line in enumerate(text.splitlines(), 1):
            code = line.split("#", 1)[0] if path.suffix in (".py", ".yaml") else line
            assert not banned.search(code), f"{path.name}:{number}: {line.strip()}"


# --- T-4641, ADR-N-053: a published chart is a jc_visualization view, framed from viz.{domain} --

VIEW = {
    "id": "6f1c2b9e-0d0a-4c35-9a51-3a3f8f6c1d20", "view_type": "jc_visualization", "package_id": "pocasie",
    "title": "temperature-24h", "description": "",
    "titles": json.dumps({"sk": "Teplota za 24 hodín", "en": "Temperature over 24 hours"}),
    "descriptions": json.dumps({"sk": "Hodinový priemer", "en": "Hourly mean"}),
    "spec": json.dumps({"$schema": "https://vega.github.io/schema/vega-lite/v5.json", "mark": "line"}),
    "source": json.dumps({"source": "temporal", "type": "WeatherObserved"}), "endpoint": "3iasem46w7d7pxxypwdfdzheyaodv3qn",
}


def test_a_chart_is_framed_by_its_view_id_alone_on_the_renderer_host(theme):
    theme.page.lang = "sk"
    viz = theme.jc_visualization(VIEW)
    assert viz["title"] == "Teplota za 24 hodín" and viz["description"] == "Hodinový priemer"
    assert viz["src"] == "https://viz.dev.example/#%s&inline&lang=sk" % VIEW["id"]
    # The frame names no spec, no Endpoint and no URL of data: the renderer reads those itself.
    assert "3iasem46" not in viz["src"] and "vega" not in viz["src"]
    assert 'src="https://viz.dev.example/#%s&lang=sk"' % VIEW["id"] in viz["code"]
    assert 'sandbox="allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox"' in viz["code"]
    theme.page.lang = "fi"
    assert theme.jc_visualization(VIEW)["title"] == "Temperature over 24 hours", "the default language's"
    theme.page.lang = "en"


def test_a_title_cannot_break_out_of_the_embed_snippet(theme):
    view = dict(VIEW, titles=json.dumps({"en": '"><script>alert(1)</script>'}))
    code = theme.jc_visualization(view)["code"]
    assert "<script>" not in code and "&quot;&gt;&lt;script&gt;" in code


def test_only_a_jc_visualization_with_a_view_id_and_a_domain_becomes_a_chart(theme):
    assert theme.jc_visualization(dict(VIEW, view_type="image_view")) is None
    assert theme.jc_visualization(dict(VIEW, id="../../x")) is None
    assert theme.jc_visualization(None) is None
    theme.BRANDING["domain"] = "evil.example/x"
    assert theme.jc_visualization(VIEW) is None and theme.jc_viz_origin() is None


def test_the_dataset_lists_the_charts_of_every_resource_and_skips_what_it_may_not_read(theme):
    views = {"r1": [{"id": "v", "view_type": "datatables_view"}, VIEW], "r2": theme.toolkit.NotAuthorized()}

    def listed(d):
        answer = views[d["id"]]
        if isinstance(answer, Exception):
            raise answer
        return answer
    theme.page.actions = {"resource_view_list": listed}
    charts = theme.jc_visualizations({"resources": [{"id": "r2"}, {"id": "r1"}]})
    assert [c["id"] for c in charts] == [VIEW["id"]]


def test_the_view_type_is_never_offered_in_ckan_and_its_config_is_checked(theme):
    plugin = theme.JcThemePlugin()
    info = plugin.info()
    assert info["name"] == "jc_visualization" and info["iframed"] is False and info["always_available"] is False
    assert plugin.can_view({"resource": {"format": "CSV"}}) is False, "never in the New view list"
    spec = info["schema"]["spec"][-1]
    assert spec(VIEW["spec"]) == VIEW["spec"]
    for bad in ("[1, 2]", "not json", json.dumps({"a": "x" * 210_000})):
        with pytest.raises(theme.toolkit.Invalid):
            spec(bad)
    slug = info["schema"]["endpoint"][-1]
    assert slug(VIEW["endpoint"]) == VIEW["endpoint"]
    # EP-144, T-4846: every field the publisher sends (jcctl VIEW_PAYLOAD) is CKAN core's or the
    # schema's; any other field is refused as 409 `__extras`.
    core = {"id", "resource_id", "view_type", "title", "description"}
    sent = core | {"jc_name", "spec", "source", "endpoint"}
    assert sent - core <= set(info["schema"]), sent - core - set(info["schema"])
    name = info["schema"]["jc_name"][-1]
    assert name("no2-now") == "no2-now" and name("auto-" + "a" * 58) == "auto-" + "a" * 58
    for bad in ("No2", "-x", "x-", "a" * 64, "../x"):
        with pytest.raises(theme.toolkit.Invalid):
            name(bad)
    for bad in ("https://evil.example/", "../x", "ABC"):
        with pytest.raises(theme.toolkit.Invalid):
            slug(bad)
    assert plugin.view_template({}, {}) == "jc/visualization.html"
    assert plugin.setup_template_variables({}, {"resource_view": VIEW})["viz"]["id"] == VIEW["id"]


def test_the_dataset_page_shows_the_charts_and_sizes_their_frames_from_the_renderer_only():
    read = (THEME / "templates__package__read.html").read_text()
    assert read.index("jc_visualizations.html") < read.index("h.jc_about(pkg)"), "the charts follow the table"
    assert "/jc-viz.js" in read and "/jc-viz.js" in (THEME / "templates__package__resource_read.html").read_text()
    figure = (THEME / "templates__jc__visualization.html").read_text()
    assert 'sandbox="allow-scripts allow-same-origin allow-downloads"' in figure and 'data-jc-viz="{{ viz.origin }}"' in figure
    script = (THEME / "public__jc-viz.js").read_text()
    assert "frame.contentWindow === event.source" in script
    assert 'frame.getAttribute("data-jc-viz") === event.origin' in script


def test_ckan_answers_cors_for_the_renderer_alone(rendered):
    ckan = next(d for d in rendered("dev") if d.get("kind") == "Deployment" and d["metadata"]["name"] == "ckan")
    env = {e["name"]: e.get("value") for c in ckan["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    assert env["CKAN__CORS__ORIGIN_WHITELIST"] == "https://viz.dev.joinedcontext.com"
    assert "CKAN__CORS__ORIGIN_ALLOW_ALL" not in env
# --- T-4639: the dataset map and its framable page -------------------------------------------


MAP_PKG = {"name": "pocasie", "title": "Počasie", "resources": [
    {"id": "o", "format": "OGCFEAT", "url": "https://dev.example/api/endpoint/abc/ogc/features/"},
    {"id": "x", "format": "OGCFEAT", "url": "https://elsewhere.example/api/endpoint/abc/ogc/features/"},
    {"id": "w", "name": "WeatherObserved", "datastore_active": True},
    {"id": "b", "name": "not a type", "datastore_active": True},
    {"id": "c", "name": "CSV"},
]}


@pytest.fixture
def mapped(theme, tmp_path, monkeypatch):
    vendor = tmp_path / "vendor"
    vendor.mkdir()
    monkeypatch.setattr(theme, "VENDOR", str(vendor))
    theme.toolkit.url_for = lambda route, **kw: "https://data.dev.example/%s/%s" % (route, kw["id"])
    theme.page.actions["datastore_search"] = lambda d: {"total": 0, "fields": []}
    return theme


def test_the_map_reads_each_table_s_type_from_the_dataset_s_own_ogc_features(mapped):
    config = mapped.jc_map(MAP_PKG)
    base = "https://dev.example/api/endpoint/abc/ogc/features/collections/WeatherObserved"
    assert config["layers"] == [{"type": "WeatherObserved", "items": base + "/items", "schema": base + "/schema",
                                 "tiles": base + "/tiles/WebMercatorQuad"}]
    assert config["lib"] == "/maplibre-gl-6.14.0/maplibre-gl.mjs"
    assert config["embed"] == "https://data.dev.example/jc_pages.map/pocasie"
    assert config["title"] == "Počasie" and set(config["t"]) >= {"map_cap", "map_close", "map_area"}
    # One table only, by its resource id; a dataset filter on a shared Endpoint stays on the items.
    assert mapped.jc_map(MAP_PKG, resource_id="b") is None
    shared = {"name": "od", "resources": [
        {"format": "OGCFEAT", "url": "https://dev.example/api/endpoint/od/ogc/features/collections/Obs/items?isPartOf=urn%3Ax"},
        {"id": "s", "name": "Obs", "datastore_active": True}, {"id": "t", "name": "Other", "datastore_active": True}]}
    layers = mapped.jc_map(shared)["layers"]
    assert layers == [{"type": "Obs", "items": "https://dev.example/api/endpoint/od/ogc/features/collections/Obs/items?isPartOf=urn%3Ax",
                       "schema": "https://dev.example/api/endpoint/od/ogc/features/collections/Obs/schema"}]


def test_no_map_without_the_library_an_ogc_address_on_our_host_or_a_table(mapped, monkeypatch):
    assert mapped.jc_map({"resources": [r for r in MAP_PKG["resources"] if r["id"] != "o"]}) is None
    assert mapped.jc_map({"resources": [r for r in MAP_PKG["resources"] if r["id"] != "w"]}) is None
    assert mapped.jc_map(None) is None
    monkeypatch.setattr(mapped, "VENDOR", "/nonexistent")
    assert mapped.jc_map(MAP_PKG) is None


@pytest.mark.parametrize("value, expected", [
    # T-4661: OpenStreetMap's own tile servers are no basemap: their policy refuses production use.
    ({"tiles": "https://tile.openstreetmap.org/{z}/{x}/{y}.png"}, {}),
    ({"style": "https://tiles.dev.example/styles/light.json"}, {"style": "https://tiles.dev.example/styles/light.json"}),
    ({"style": "https://tiles.evil.example/styles/light.json"}, {}),
    ({"tiles": "https://a.tile.example/{z}/{x}/{y}.png"}, {}),
    ("not json", {}),
    ([], {}),
])
def test_the_basemap_is_the_tiles_host_s_style_or_none(theme, monkeypatch, value, expected):
    monkeypatch.setenv("JC_MAP_BASEMAP", value if isinstance(value, str) else json.dumps(value))
    assert theme._basemap() == expected


def test_the_map_page_is_a_404_for_an_unknown_or_unmapped_dataset(mapped):
    class Abort(Exception):
        pass

    def abort(code):
        raise Abort(code)

    mapped.toolkit.abort = abort
    mapped.toolkit.render = lambda template, extra: (template, extra)
    mapped.page.actions["package_show"] = mapped.toolkit.ObjectNotFound()
    with pytest.raises(Abort):
        mapped.jc_map_page("nope")
    mapped.page.actions["package_show"] = lambda d: {"name": d["id"], "resources": []}
    with pytest.raises(Abort):
        mapped.jc_map_page("empty")
    mapped.page.actions["package_show"] = lambda d: MAP_PKG
    mapped.toolkit.g = types.SimpleNamespace()
    (template, extra), status, headers = mapped.jc_map_page("pocasie", "w")
    assert template == "jc/map.html" and extra["jc_map_config"]["layers"][0]["type"] == "WeatherObserved"
    # The page owns its policy (OPS-34): any site may frame it, nothing else is loosened (T-5147).
    assert status == 200 and headers == {"Content-Security-Policy": mapped._csp(mapped.jc_csp_nonce(), "*")}
    assert "frame-ancestors *;" in headers["Content-Security-Policy"]


def test_the_map_page_may_be_framed_anywhere_and_carries_no_cookie():
    plugins = yaml.safe_load((PROJECT_ROOT / "components/ckan/apisix-plugins.yaml").read_text())["ckan-map"]["plugins"]
    assert "X-Frame-Options" not in plugins["response-rewrite"]["headers"]["set"]
    assert plugins["response-rewrite"]["headers"]["remove"] == ["Set-Cookie"]
    route = yaml.safe_load((PROJECT_ROOT / "components/ckan/apisix-routes.yaml").read_text())["ckan-map"]
    pattern = re.compile(route["vars"][0][2])
    for path in ("/map/pocasie", "/sk/map/pocasie", "/en_GB/map/pocasie/6c9b58e0"):
        assert pattern.search(path), path
    for path in ("/dataset/pocasie/map", "/map/", "/map/a/b/c", "/xmap/a", "/sk/dataset/map/a"):
        assert not pattern.search(path), path
    page = (THEME / "templates__jc__map.html").read_text()
    assert "<form" not in page and "csrf" not in page.lower() and "{% extends" not in page


def test_the_map_library_is_pinned_by_checksum_and_served_where_the_theme_asks():
    dockerfile = (PROJECT_ROOT / "images/ckan/Dockerfile").read_text()
    assert re.search(r"ADD --checksum=sha256:[0-9a-f]{64} \\\n    https://registry\.npmjs\.org/maplibre-gl/-/maplibre-gl-6\.14\.0\.tgz", dockerfile)
    assert "/srv/app/vendor/maplibre-gl-6.14.0/" in dockerfile and "MapLibre GL JS 6.14.0 (BSD-3-Clause" in dockerfile
    plugin = (THEME / "plugin.py").read_text()
    assert 'MAPLIBRE = "/maplibre-gl-6.14.0/"' in plugin and 'VENDOR = "/srv/app/vendor"' in plugin
    snippet = (THEME / "templates__snippets__jc_map.html").read_text()
    assert "url_for_static('/maplibre-gl-6.14.0/maplibre-gl.css')" in snippet


def test_the_map_card_stays_hidden_until_a_feature_has_a_geometry():
    page = (THEME / "templates__package__read.html").read_text()
    assert '<details class="jc-table jc-map-card" open hidden>' in page
    assert "<script type=\"module\" src=\"{{ h.url_for_static('/jc-map.js') }}\"></script>" in page
    script = (THEME / "public__jc-map.js").read_text()
    assert script.index("if (!latest.features.length) {") < script.index("card.hidden = false")
    # T-4661: no probe of one feature before the page: one round trip before the first point.
    assert 'limit: "1"' not in script and "probe(" not in script
    # Values reach the page as text only, and a `next` link never leaves the layer's address.
    assert "innerHTML" not in script and "insertAdjacentHTML" not in script
    assert "candidate.origin === base.origin && candidate.pathname === base.pathname" in script
    assert 'credentials: "omit"' in script


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_the_panel_reads_like_the_data_and_measures_lines_and_areas():
    script = (THEME / "public__jc-map.js").resolve().as_uri()
    code = """
import { describe, length, area } from %s;
import { readFileSync } from "node:fs";
const t = { map_length: "Dĺžka", map_area: "Plocha" };
const schema = { temperature: { "x-ogc-unit": "Cel", description: "Air temperature" }, windDirection: { "x-ogc-unit": "deg" },
  dateObserved: { format: "date-time" } };
const point = { id: "urn:ngsi-ld:W:1", geometry: { type: "Point", coordinates: [19, 48] }, properties: {
  name: { languageMap: { en: "Donovaly station", sk: "Stanica Donovaly" } }, temperature: 12.4, windDirection: 38.5,
  dateObserved: new Date(Date.now() - 5 * 60000).toISOString(), refParent: "urn:x", other: "urn:ngsi-ld:A:1",
  nested: { a: 1 }, type: "W", id: "urn:x", _layer: 0, _id: "urn:ngsi-ld:W:1", _n: 0, note: "<b>x</b>" } };
const line = { geometry: { type: "LineString", coordinates: [[19, 48], [19, 48.01]] }, properties: { _id: "urn:ngsi-ld:R:road-7" } };
const square = { geometry: { type: "Polygon", coordinates: [[[19, 48], [19.01, 48], [19.01, 48.01], [19, 48.01], [19, 48]]] }, properties: {} };
const enums = JSON.parse(readFileSync(%s, "utf8"));
const layers = [{ fields: [
  { a: "temperature", l: { sk: "Teplota", en: "Temperature" }, s: true },
  { a: "windDirection", l: { sk: "Smer vetra" }, s: true },
  { a: "dateObserved", l: { sk: "Dátum pozorovania" }, s: false },
  { a: "note", l: {}, s: false } ] }];
console.log(JSON.stringify({
  point: describe(point, schema, { lang: "sk", t, enums, layers }),
  line: describe(line, {}, { lang: "en", t }),
  square: describe(square, {}, { lang: "en", t }),
  length: length(line.geometry), area: area(square.geometry) }));
""" % (json.dumps(script), json.dumps(str(THEME / "public__jc-enums.json")))
    done = subprocess.run(["node", "--input-type=module", "-e", code], capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    out = json.loads(done.stdout)
    rows = dict((label, value) for label, value, _, _ in out["point"]["rows"] + out["point"]["more"])
    # The key attributes come first; the rest folds under "Viac" (T-4672).
    assert [r[0] for r in out["point"]["rows"]] == ["Teplota", "Smer vetra"]
    assert out["point"]["title"] == "Stanica Donovaly"
    assert rows["Teplota"] == "12,4\u00a0°C" and rows["Smer vetra"] == "38,5°"
    assert rows["Dátum pozorovania"] == "pred 5 minútami"
    assert rows["Note"] == "<b>x</b>"  # shown as text by textContent, never parsed
    assert not {"refParent", "Other", "Nested", "Typ", "id", "_layer", "_id", "_n", "Názov"} & set(rows)
    assert out["line"]["title"] == "road-7"
    assert out["line"]["rows"][0][0] == "Dĺžka" and re.fullmatch(r"1\.1\d\u00a0km", out["line"]["rows"][0][1])
    assert 1100 < out["length"] < 1125  # 0.01° of latitude
    assert 0.8e6 < out["area"] < 0.85e6 and out["square"]["rows"][0][1].endswith("\u00a0ha")


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_the_tile_address_stays_on_the_collection_s_own_tiles_path():
    # T-5189: the map reads a tileset's item template only under the layer's own tileset address.
    script = (THEME / "public__jc-map.js").resolve().as_uri()
    at = "https://api.dev.example/api/endpoint/abc/ogc/features/collections/Bike/tiles/WebMercatorQuad"
    code = """
import { templateOf, itemUrl } from %s;
const at = %s;
const mvt = "application/vnd.mapbox-vector-tile";
const of = (href, type = mvt) => templateOf({ links: [{ rel: "self", type: "application/json", href: at }, { rel: "item", type, templated: true, href }] }, at);
console.log(JSON.stringify({
  own: of(at + "/{tileMatrix}/{tileRow}/{tileCol}"),
  elsewhere: of("https://evil.example/t/{tileMatrix}/{tileRow}/{tileCol}"),
  broker: of("http://antares:9471/ngsi-ld/ogc/collections/Bike/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
  sibling: of(at.replace("/Bike/", "/Other/") + "/{tileMatrix}/{tileRow}/{tileCol}"),
  query: of(at + "/{tileMatrix}/{tileRow}/{tileCol}?f=mvt"),
  order: of(at + "/{tileMatrix}/{tileCol}/{tileRow}"),
  json: of(at + "/{tileMatrix}/{tileRow}/{tileCol}", "application/json"),
  none: templateOf({}, at),
  item: itemUrl(at.replace("/tiles/WebMercatorQuad", "/items"), "urn:ngsi-ld:Bike:a/b"),
}));
""" % (json.dumps(script), json.dumps(at))
    done = subprocess.run(["node", "--input-type=module", "-e", code], capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    out = json.loads(done.stdout)
    assert out["own"] == at + "/{z}/{y}/{x}"
    assert [k for k, v in out.items() if v is None] == ["elsewhere", "broker", "sibling", "query", "order", "json", "none"]
    assert out["item"] == at.replace("/tiles/WebMercatorQuad", "/items") + "/urn%3Angsi-ld%3ABike%3Aa%2Fb"


# T-4672: the popup reads in the order, with the labels and key attributes, the publisher wrote
# into the DataStore table from the data model; the theme keeps only its words for code values.
def test_the_theme_keeps_only_its_enum_words():
    words = json.loads((THEME / "public__jc-enums.json").read_text())
    assert set(words) == {"$comment", "enums"}
    assert words["enums"]["museumsGalleriesLibraries"]["sk"] == "Múzeá, galérie, knižnice"
    assert not (THEME / "public__jc-field-order.json").exists(), "no copy of jc-core's heuristic"


# What the BB events table carries after `jcctl publish`, as plugin._display_fields gives it.
EVENT_FIELDS = [
    {"a": "startDate", "l": {"sk": "Začiatok", "en": "Start"}, "s": True},
    {"a": "address", "l": {"sk": "Adresa", "en": "Address"}, "s": True},
    {"a": "endDate", "l": {"sk": "Koniec", "en": "End"}, "s": False},
    {"a": "eventCategory", "l": {"sk": "Kategória", "en": "Category"}, "s": False},
    {"a": "url", "l": {"sk": "Odkaz", "en": "Link"}, "s": False},
    {"a": "source", "l": {"sk": "Zdroj", "en": "Source"}, "s": False},
    {"a": "dataProvider", "l": {"sk": "Poskytovateľ", "en": "Data provider"}, "s": False},
    {"a": "dateModified", "l": {"sk": "Zmenené", "en": "Modified"}, "s": False},
]


def _describe(feature, lang="sk", fields=EVENT_FIELDS):
    script = (THEME / "public__jc-map.js").resolve().as_uri()
    code = """
import { describe } from %s;
import { readFileSync } from "node:fs";
const enums = JSON.parse(readFileSync(%s, "utf8"));
console.log(JSON.stringify(describe(%s, {}, { lang: %s, t: {}, enums, layers: [{ fields: %s }] })));
""" % (json.dumps(script), json.dumps(str(THEME / "public__jc-enums.json")), json.dumps(feature), json.dumps(lang),
       json.dumps(fields))
    done = subprocess.run(["node", "--input-type=module", "-e", code], capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    return json.loads(done.stdout)


EVENT = {"geometry": {"type": "Point", "coordinates": [19.15, 48.73]}, "properties": {
    "dataProvider": "Mesto Banská Bystrica", "dateModified": "2026-10-09T10:42:00Z", "endDate": "2026-11-10",
    "eventCategory": "museumsGalleriesLibraries", "name": {"languageMap": {"sk": "Výstava v Bábkarskej dielni"}},
    "source": "https://www.banskabystrica.sk/podujatia/", "startDate": "2026-10-15", "startTime": "18:00",
    "url": "https://www.banskabystrica.sk/podujatie/123", "price": 5, "_layer": 0, "_id": "urn:ngsi-ld:Event:1", "_n": 0}}


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_an_event_shows_its_key_attributes_then_the_rest_in_the_tables_order_and_labels():
    out = _describe(EVENT)
    assert out["title"] == "Výstava v Bábkarskej dielni"
    # Key attributes first, the rest under "Viac"; `price` is no column of the table: not shown.
    assert [row[0] for row in out["rows"]] == ["Začiatok"]
    assert [row[0] for row in out["more"]] == ["Koniec", "Kategória", "Odkaz", "Zdroj", "Poskytovateľ", "Zmenené"]
    rows = {row[0]: row for row in out["rows"] + out["more"]}
    # The start takes its time along; the time is no row of its own.
    assert rows["Začiatok"][1] == "15. 10. 2026, 18:00" and "Čas" not in rows
    assert rows["Koniec"][1] == "10. 11. 2026"
    assert rows["Kategória"][1] == "Múzeá, galérie, knižnice"
    assert rows["Odkaz"][1:] == ["https://www.banskabystrica.sk/podujatie/123", "", "link"]
    assert [rows[k][3] for k in ("Zdroj", "Poskytovateľ", "Zmenené")] == ["meta", "meta", "meta"]
    # On an English page the same table reads in English (T-4648).
    english = _describe(EVENT, "en")
    assert [row[0] for row in english["rows"] + english["more"]][:3] == ["Start", "End", "Category"]


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_only_an_http_address_is_a_link_and_an_unknown_code_reads_as_words():
    feature = {"geometry": None, "properties": {"url": "javascript:alert(1)", "eventCategory": "openAirCinema",
                                                "startDate": "2026-10-15T18:00:00", "startTime": "19:00", "_layer": 0}}
    out = _describe(feature, "en")
    rows = {row[0]: row for row in out["rows"] + out["more"]}
    assert rows["Link"][1:] == ["javascript:alert(1)", "", ""]
    assert rows["Category"][1] == "Open air cinema"
    # A start that has its time keeps it, and the separate time is not repeated.
    assert "Start time" not in rows and rows["Start"][1].startswith("Oct 15, 2026")


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_without_the_tables_order_the_panel_still_shows_every_value():
    out = _describe(EVENT, "sk", fields=[])
    assert len(out["rows"]) == 8 and out["more"] == [] and out["rows"][0][0] == "Data provider"


def test_the_map_layer_carries_the_order_labels_and_key_flags_of_its_table(mapped):
    mapped.page.actions["datastore_search"] = lambda d: {"total": 2, "fields": [
        {"id": "entity_id", "type": "text", "info": {"label": "Identifikátor"}},
        {"id": "name.languageMap.sk", "type": "text", "info": {"label": "Názov", "labels": {"sk": "Názov", "en": "Name"}, "summary": True}},
        {"id": "address.value.streetAddress", "type": "text", "info": {"label": "Ulica", "labels": {"sk": "Ulica"}}},
        {"id": "address.value", "type": "text", "info": {"label": "Adresa", "labels": {"sk": "Adresa", "en": "Address"}}},
        {"id": "_id", "type": "int"},
    ]}
    assert mapped.jc_map(MAP_PKG)["layers"][0]["fields"] == [
        {"a": "name", "l": {"sk": "Názov", "en": "Name"}, "s": True},
        {"a": "address", "l": {"sk": "Adresa", "en": "Address"}, "s": False},
    ]
    # A table the publisher wrote before T-4672 carries no labels: no fields, the popup's fallback.
    mapped.page.actions["datastore_search"] = lambda d: {"fields": [{"id": "name.value", "type": "text", "info": {"label": "Názov"}}]}
    assert "fields" not in mapped.jc_map(MAP_PKG)["layers"][0]
    mapped.page.actions["datastore_search"] = mapped.toolkit.NotAuthorized()
    assert "fields" not in mapped.jc_map(MAP_PKG)["layers"][0]


def test_the_map_page_loads_the_labels_and_the_basemap_from_our_hosts_only():
    plugin = (THEME / "plugin.py").read_text()
    assert '"enums": "/jc-enums.json"' in plugin and 'PMTILES = "/pmtiles-4.5.0.js"' in plugin
    assert "openstreetmap" not in (THEME / "public__jc-map.js").read_text()
    import hashlib
    pmtiles = (THEME / "public__pmtiles-4.5.0.js").read_bytes()
    # pmtiles 4.5.0 (BSD-3-Clause), package/dist/pmtiles.js as npm publishes it; the viz map's copy.
    assert hashlib.sha256(pmtiles).hexdigest() == "caf981bc46f6327ee7e65d5dc964d89d38a69f60edca2bd4c5c890c21b554c6c"
    snippet = (THEME / "templates__snippets__jc_map.html").read_text()
    assert "{% if jc_map_config.pmtiles %}<script src=\"{{ h.url_for_static(jc_map_config.pmtiles) }}\"></script>{% endif %}" in snippet


def test_no_ckan_values_point_a_map_at_openstreetmap_s_tile_servers():
    files = [PROJECT_ROOT / "charts/ckan/values.yaml", *sorted((PROJECT_ROOT / "components/ckan").rglob("*.y*ml*"))]
    for path in files:
        assert "tile.openstreetmap.org" not in path.read_text(), path
    dev = (PROJECT_ROOT / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    assert "style: https://tiles.{{ .Values.global.domain }}/styles/light-sk.json" in dev


def test_the_table_headers_are_the_publisher_s_labels_and_id_reads_as_a_row_number():
    css = (THEME / "public__jc-theme.css").read_text()
    assert 'body.dt-view #dtprv thead th[data-name="_id"] { font-size: 0; }' in css
    assert 'content: "#"' in css
    # CKAN shows `info.label` while data_dictionary_labels is on, its default; nothing turns it off.
    for path in [PROJECT_ROOT / "charts/ckan/values.yaml", *(PROJECT_ROOT / "components/ckan").rglob("*.y*ml*"),
                 *(PROJECT_ROOT / "charts/ckan/templates").glob("*.yaml")]:
        assert "data_dictionary_labels" not in path.read_text().lower(), path


# T-4643: Gemini's card behind its flag, and one guide per assistant for a non-technical reader.
def test_gemini_has_a_card_and_a_guide_only_while_its_flag_is_on(theme, monkeypatch):
    assert theme.jc_assistant_guide("gemini") is None
    monkeypatch.setenv("JC_ASSISTANT_GEMINI", "1")
    assert theme.jc_assistant_guide("gemini") is None, "only the chart's literal true turns it on"
    monkeypatch.setenv("JC_ASSISTANT_GEMINI", "true")
    cards = theme.jc_assistants()["cards"]
    assert [card["id"] for card in cards] == ["chatgpt", "claude", "grok", "gemini"]
    assert theme.jc_assistant_guide("gemini")["name"] == "Gemini"
    html = render("jc/assistants.html", theme)
    assert 'id="jc-card-gemini"' in html and 'href="/assistants/gemini"' in html


def test_an_unknown_assistant_or_no_hub_has_no_guide_and_its_page_is_a_404(theme):
    theme.toolkit.abort = lambda code: ("abort", code)
    theme.toolkit.render = lambda template, extra: (template, extra)
    for id in ("gemini", "copilot", "", "../chatgpt", "__class__"):
        assert theme.jc_assistant_guide(id) is None and theme.jc_guide_page(id) == ("abort", 404), id
    assert theme.jc_guide_page("claude")[0] == "jc/guide.html"
    theme.BRANDING["domain"] = "dev.example/evil"
    assert theme.jc_assistant_guide("claude") is None


MAKERS = {"chatgpt": ("chatgpt.com", "help.openai.com"), "claude": ("claude.ai", "support.claude.com"),
          "grok": ("grok.com", "docs.x.ai"), "gemini": ("gemini.google.com", "support.google.com")}
# "MCP server" is the one protocol word a guide uses: what the reader adds (T-4735, the owner).
TECHNICAL = re.compile(r"\b(MCP(?![ -][Ss]erver)|OAuth|endpoint|JSON)\b", re.I)


def test_every_guide_is_written_in_four_languages_with_the_same_steps(theme):
    assert list(theme.GUIDES) == list(MAKERS)
    for id, guide in theme.GUIDES.items():
        assert {"sk", "en", "cs", "de"} <= set(guide) and re.fullmatch(r"\d{4}-\d{2}-\d{2}", guide["checked"]), id
        counts = {lang: len(guide[lang]["steps"]) for lang in ("sk", "en", "cs", "de")}
        assert len(set(counts.values())) == 1 and 3 <= counts["en"] <= 8, (id, counts)
        assert 0 <= guide["paste"] < counts["en"], id
        for lang in ("sk", "en", "cs", "de"):
            words = guide[lang]
            assert 1 <= len(words["who"]) <= 2 and 2 <= len(words["trouble"]) <= 3, (id, lang)
            # The step that asks for the address is the one its box follows.
            assert words["steps"][guide["paste"]].endswith(":"), (id, lang)
            text = " ".join(words["who"] + words["steps"] + [t for pair in words["trouble"] for t in pair])
            assert not TECHNICAL.search(text), (id, lang, TECHNICAL.search(text))
            # T-4628: the owner read English store words inside the Slovak steps.
            if lang in ("sk", "cs"):
                assert not re.search(r"Add custom|Create as|New Connector|Connected Apps", text), (id, lang)


@pytest.mark.parametrize("lang, checked", [("sk", "Overené dňa"), ("en", "Checked on"), ("cs", "Ověřeno dne"), ("de", "Geprüft am")])
@pytest.mark.parametrize("id", list(MAKERS))
def test_each_guide_page_reads_top_to_bottom_without_a_technical_word(theme, monkeypatch, id, lang, checked):
    monkeypatch.setenv("JC_ASSISTANT_GEMINI", "true")
    theme.page.lang = lang
    guide = theme.jc_assistant_guide(id)
    html = render("jc/guide.html", theme, jc_guide=guide)
    steps = re.search(r'<ol class="jc-guide-steps">(.*?)</ol>', html, re.S).group(1)
    items = re.findall(r"<li>(.*?)</li>", steps, re.S)
    assert len(items) == len(theme.GUIDES[id]["en"]["steps"]) and "{name}" not in html
    # The address once, in a box inside the step that asks for it, with the one copy button.
    hub = "https://api.dev.example/api/mcp"
    boxed = [i for i, item in enumerate(items) if 'id="jc-store-url"' in item]
    assert boxed == [theme.GUIDES[id]["paste"]] and html.replace("<wbr>", "").count(hub) == 1
    # The address's copy button and the note's example question's (T-4735).
    assert html.count("<button") == 2 and html.count("data-jc-copy=") == 2
    assert f"{checked} {theme.GUIDES[id]['checked']}" in html
    # Links only to the maker's own domains, and back to our own pages.
    for href in re.findall(r'href="([^"]*)"', html):
        host = urlsplit(href).hostname
        assert host is None and href.startswith("/assistants") or host in MAKERS[id], href
    assert theme.GUIDES[id]["help"] in html and "<script>" not in html
    # What a reader reads: the page's words, without the address box and the help page's address.
    text = re.sub(r'<span id="jc-store-url".*?</span>', "", html, flags=re.S).replace(theme.GUIDES[id]["help"], "")
    text = re.sub(r"<[^>]+>", " ", text)
    assert not TECHNICAL.search(text), TECHNICAL.search(text)
    assert guide["example"] in html and len(re.findall(r"<dt>", html)) == len(guide["trouble"])
    # T-4735: no store button, and our server is never a plugin; ChatGPT's page is named Plugins.
    assert "btn-primary" not in html and not re.search(r"Pridať do|Add to|Přidat do|Zu \w+ hinzufügen", html)
    assert not re.search(r"doplnok|doplněk|plugin", text.replace("Plugins", ""), re.I), id


def test_a_guide_asks_the_instance_s_own_question_first(theme, monkeypatch):
    theme.BRANDING["samples"] = {"sk": ["Aké podujatia sú zajtra v Banskej Bystrici?"]}
    theme.page.lang = "sk"
    assert theme.jc_assistant_guide("grok")["example"] == "Použi Joinedcontext Open Data: Aké podujatia sú zajtra v Banskej Bystrici?"
    theme.BRANDING["samples"] = {}
    assert theme.jc_assistant_guide("grok")["example"] == "Použi Joinedcontext Open Data: Aké datasety o kvalite ovzdušia máte?"
    # Gemini answers in English only, so its guide asks in English on a Slovak page too.
    monkeypatch.setenv("JC_ASSISTANT_GEMINI", "true")
    assert theme.jc_assistant_guide("gemini")["example"] == "Use Joinedcontext Open Data: Which datasets about air quality do you have?"


def test_the_guide_link_opens_the_whole_card_and_wraps_on_a_phone():
    css = (THEME / "public__jc-theme.css").read_text()
    assert ".jc-card-guide" not in css
    assert ".jc-card-links { margin: auto 0 0; display: flex; flex-wrap: wrap;" in css
    assert ".jc-guide-checked { color: var(--jc-muted); overflow-wrap: anywhere; }" in css


# T-4659: the assistants page lists the websites the hub's search_documents reads.
def test_the_assistants_page_lists_the_hub_s_websites_and_drops_a_malformed_entry(theme, monkeypatch):
    monkeypatch.setenv("JC_ASSISTANT_SOURCES", json.dumps([
        {"publisher": "banskabystrica", "url": "https://www.banskabystrica.sk/", "languages": ["sk"]},
        {"publisher": "helsinki", "url": "https://www.hel.fi/", "languages": ["fi", "sv", "en"]},
        {"publisher": "evil", "url": "javascript:alert(1)", "languages": []},
        {"publisher": "plain", "url": "http://example.org/", "languages": []},
        {"url": "https://nopublisher.example/"},
        "not an entry",
    ]))
    assert theme.jc_assistants()["sources"] == [
        {"site": "www.banskabystrica.sk", "url": "https://www.banskabystrica.sk/", "publisher": "banskabystrica", "languages": "sk"},
        {"site": "www.hel.fi", "url": "https://www.hel.fi/", "publisher": "helsinki", "languages": "fi, sv, en"},
    ]
    monkeypatch.setenv("JC_ASSISTANT_SOURCES", "{not json")
    assert theme.jc_assistant_sources() == []
    monkeypatch.delenv("JC_ASSISTANT_SOURCES")
    assert theme.jc_assistant_sources() == []
    template = (THEME / "templates__jc__assistants.html").read_text()
    assert "{% if page.sources %}" in template and 'aria-labelledby="jc-assistant-sources"' in template
    for key in ("assistants_sources", "assistants_sources_lead", "assistants_source_languages"):
        assert set(theme.STRINGS[key]) == LANGUAGES, key


def test_dev_lists_the_hub_s_knowledge_sources_as_the_seed_declares_them():
    """The page's list is the gateway's hub list, each site the seed source's first start URL and
    its languages, so the page never names a site the hub does not search."""
    root = Path(__file__).resolve().parent.parent
    values = (root / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    block = values[values.index("\nassistants:\n"):]
    listed = yaml.safe_load(block.split("\n\n")[0])["assistants"]["sources"]
    gateway = (root / "components/context-gateway/values/gateway/base-values.yaml.gotmpl").read_text()
    (hub,) = [line.split(":", 1)[1].strip().strip('"') for line in gateway.splitlines()
              if "JC_GATEWAY_HUB_KNOWLEDGE_SOURCES:" in line]
    assert [f"{s['publisher']}/web" for s in listed] == hub.split(",")
    for s in listed:
        seed = yaml.safe_load((root / f"components/context-gateway/seed/{s['publisher']}/{s['publisher']}-knowledge-web.yaml").read_text())
        assert s["url"] == seed["spec"]["startUrls"][0], s
        assert s["languages"] == seed["spec"]["languages"], s


LABELLED_SEEDS = ["banskabystrica/banskabystrica-verejne", "helsinki/helsinki"]


@pytest.mark.parametrize("seed", LABELLED_SEEDS)
def test_every_attribute_the_map_popup_shows_has_a_label_in_sk_cs_en_and_de(seed):
    """T-4648: the popup reads the model's labels in the page's language, so an attribute without
    one would read in English words on a Slovak, Czech or German page."""
    model = yaml.safe_load((PROJECT_ROOT / f"components/context-gateway/seed/{seed}.linkml.yaml").read_text())
    slots = model.get("slots") or {}
    missing = []
    for name, cls in (model.get("classes") or {}).items():
        usage = cls.get("slot_usage") or {}

        def label(slot):
            for node in (usage.get(slot) or {}, slots.get(slot) or {}):
                found = (node.get("annotations") or {}).get("label")
                if found:
                    return found["value"]
            return None

        # A class the model gives labels of its own; the others keep their attribute names for now.
        if not any((node.get("annotations") or {}).get("label") for node in usage.values()):
            continue
        for slot in cls.get("slots") or []:
            if slot == "location" or re.match(r"^ref[A-Z]", slot) or (slots.get(slot) or {}).get("range") in model.get("classes", {}):
                continue
            labels = label(slot) or {}
            gaps = {"sk", "cs", "en", "de"} - set(labels)
            if gaps:
                missing.append(f"{name}.{slot}: {sorted(gaps)}")
    assert not missing, missing


# --- T-5059: an Endpoint's dataset opens the live explorer on viz.{domain}; nothing is copied ----

EXPLORED = {"extras": [{"key": "endpoint", "value": "https://api.dev.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/"}]}


def test_the_explorer_frames_the_endpoint_by_its_slug_and_the_resource_by_its_type(theme):
    theme.page.lang = "sk"
    explorer = theme.jc_explorer(EXPLORED)
    assert explorer["src"] == "https://viz.dev.example/explore.html#scsd2eehkx42n53z2zyd6vshfh7s7irf&inline&lang=sk&theme=light"
    assert explorer["origin"] == "https://viz.dev.example" and explorer["title"] == "Živé dáta"
    odata = {"url": "https://api.dev.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/odata/v4/BikeHireDockingStation"}
    assert theme.jc_explorer(EXPLORED, odata)["src"].endswith("&type=BikeHireDockingStation")
    table = {"url": "https://opendata.dev.example/datastore/dump/x", "datastore_active": True, "name": "WeatherObserved"}
    assert theme.jc_explorer(EXPLORED, table)["src"].endswith("&type=WeatherObserved")
    # A name that is no entity type never reaches the frame's address.
    odd = {"url": "https://opendata.dev.example/x", "datastore_active": True, "name": "Teploty & tlak"}
    assert "&type=" not in theme.jc_explorer(EXPLORED, odd)["src"]
    theme.page.lang = "fi"


def test_the_explorer_frame_names_the_dataset_for_its_embed_code(theme):
    """T-5129: the explorer's Embed code names the dataset, whose title and licence the embed shows."""
    theme.page.lang = "sk"
    named = dict(EXPLORED, name="bike-stations")
    assert theme.jc_explorer(named)["src"] == "https://viz.dev.example/explore.html#scsd2eehkx42n53z2zyd6vshfh7s7irf&inline&lang=sk&theme=light&dataset=bike-stations"
    assert "&dataset=" not in theme.jc_explorer(dict(EXPLORED, name="Bike & stations"))["src"]
    theme.page.lang = "fi"
    snippet = (THEME / "templates__package__snippets__jc_explorer.html").read_text()
    assert 'allow="clipboard-write"' in snippet and "allow-forms" not in snippet


@pytest.mark.parametrize("endpoint", [
    "https://api.evil.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/",
    "https://broker.dev.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/",
    "https://api.dev.example/api/endpoint/SHORT/",
    "http://api.dev.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/",
    "https://api.dev.example.evil.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/",
])
def test_the_explorer_is_left_out_for_an_endpoint_off_the_platform_api(theme, endpoint):
    assert theme.jc_explorer({"extras": [{"key": "endpoint", "value": endpoint}]}) is None
    assert theme.jc_explorer({}) is None


def test_an_endpoint_s_dataset_page_opens_the_explorer_and_asks_no_datastore():
    read = (THEME / "templates__package__read.html").read_text()
    assert "{% set previews = [] if explorer else h.jc_previews(pkg) %}" in read
    assert read.index("h.jc_explorer(pkg)") < read.index("h.jc_about(pkg)")
    snippet = (THEME / "templates__package__snippets__jc_explorer.html").read_text()
    assert 'data-jc-viz="{{ explorer.origin }}"' in snippet and 'title="{{ explorer.title }}"' in snippet
    assert 'referrerpolicy="no-referrer"' in snippet and "allow-top-navigation" not in snippet
    resource = (THEME / "templates__package__resource_read.html").read_text()
    assert "h.jc_explorer(pkg, res)" in resource and "{% if not explorer and (resource_views" in resource
    assert "{% set preview = none if explorer else h.jc_preview(res) %}" in resource
    assert "jc-viz.js" in read and "jc-viz.js" in resource, "the frame's height comes from jc-viz.js"


def test_the_publishers_picture_link_is_not_a_second_nameless_stop_for_a_keyboard():
    """T-5108: axe link-name (serious) on every dataset page; the name link beside it carries the name."""
    snippet = (THEME / "templates__package__snippets__jc_publisher.html").read_text()
    image = re.search(r'<a [^>]*class="jc-publisher-image"[^>]*>', snippet).group(0)
    assert 'tabindex="-1"' in image and 'aria-hidden="true"' in image
    assert 'class="jc-publisher-name">{{ organization.title or organization.name }}</a>' in snippet


def test_the_accessibility_statement_names_status_gaps_method_date_and_enforcement(theme):
    """T-5108: the parts EU decision 2018/1523 asks of a statement, in Slovak and English."""
    for lang in ("en", "sk"):
        theme.page.lang = lang
        page = theme.jc_page("accessibility")
        text = " ".join(p for s in page["sections"] for p in s["paragraphs"])
        assert "95/2019" in text and "78/2020" in text and "WCAG 2.1" in text and "2026" in text
        assert len(page["sections"]) == 8 and len(page["sections"][3]["paragraphs"]) == 4
    footer = (THEME / "footer.html").read_text()
    assert "'accessibility')" in footer


def test_an_api_resource_links_the_portal_developer_page_of_its_endpoint(theme):
    """T-5111: the resource page of a public Endpoint links its developer page on the Portal."""
    theme.page.lang = "sk"
    assert theme.jc_developer_page(EXPLORED) == "https://portal.dev.example/d/scsd2eehkx42n53z2zyd6vshfh7s7irf?lang=sk"
    theme.page.lang = "fi"
    for endpoint in ("https://api.evil.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/",
                     "https://api.dev.example/api/endpoint/SHORT/",
                     "http://api.dev.example/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/"):
        assert theme.jc_developer_page({"extras": [{"key": "endpoint", "value": endpoint}]}) is None
    assert theme.jc_developer_page({}) is None
    resource = (THEME / "templates__package__resource_read.html").read_text()
    examples = resource[resource.index('<section class="jc-examples"'):resource.index("</section>", resource.index('<section class="jc-examples"'))]
    assert "h.jc_developer_page(pkg)" in examples and "h.jc_t('developer_page')" in examples


# T-5123: datasets findable from outside.
check_spec = importlib.util.spec_from_file_location("check_schema_org", PROJECT_ROOT / "scripts/check-schema-org.py")
check_schema_org = importlib.util.module_from_spec(check_spec)
check_spec.loader.exec_module(check_schema_org)
DEV_DATASETS = json.loads((PROJECT_ROOT / "tests/fixtures/ckan-schema-org/dev-datasets.json").read_text())


def dev_harvest_regions():
    """`harvest_source` -> spatialUri of dev's harvest sources, as the chart names them."""
    values = (PROJECT_ROOT / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    sources = yaml.safe_load(values.replace('{{"{{"}}', "{{").replace('{{"}}"}}', "}}"))["harvest"]["sources"]
    return {f"{s.get('kind', 'nkod')}-{s['name']}": s["spatialUri"] for s in sources}


def after_next_harvest(pkg):
    """The dev dataset (package_search, 2026-10-22) as the next harvest writes it: the harvests
    wrote no spatial_uri before T-5123."""
    pkg = json.loads(json.dumps(pkg))
    extras = {e["key"]: e["value"] for e in pkg["extras"]}
    if "harvest_source" in extras and "spatial_uri" not in extras:
        pkg["extras"].append({"key": "spatial_uri", "value": dev_harvest_regions()[extras["harvest_source"]]})
    return pkg


@pytest.fixture
def site(theme, monkeypatch):
    monkeypatch.setattr(theme.toolkit, "config", {"ckan.site_url": "https://opendata.dev.example/"}, raising=False)
    return theme


def test_every_dev_dataset_is_a_valid_schema_org_dataset(site):
    """Done when: each public dataset's JSON-LD passes the validator with 0 problems, all 102 of dev."""
    assert len(DEV_DATASETS) == 102
    failed = {}
    for pkg in DEV_DATASETS:
        record = json.loads(site.jc_schema_org(after_next_harvest(pkg)))
        found = check_schema_org.problems(record)
        if found:
            failed[pkg["name"]] = found
        assert record["url"] == "https://opendata.dev.example/dataset/" + pkg["name"]
    assert failed == {}


def test_a_harvested_dataset_without_its_region_is_named_by_the_validator(site):
    """The gap the harvests had: the validator is not vacuous."""
    pkg = next(p for p in DEV_DATASETS if p["name"] == "bbsk-zoznam-lekarni-v-bbk-c564de")
    assert check_schema_org.problems(json.loads(site.jc_schema_org(pkg))) == ["spatialCoverage missing or not a named Place"]
    assert set(check_schema_org.problems({"@context": "https://schema.org/", "@type": "Dataset"})) == {
        "name missing", "description missing or not 50 to 5000 characters", "license missing or not a URL",
        "creator missing or without @type and name", "distribution missing",
        "spatialCoverage missing or not a named Place", "temporalCoverage missing or not an ISO 8601 date or interval"}


def test_the_record_names_the_region_licence_publisher_and_each_door(site):
    site.page.lang = "sk"
    pkg = next(p for p in DEV_DATASETS if p["name"] == "banskabystrica-pocasie")
    record = site.jc_schema_org_dict(pkg)
    assert record["spatialCoverage"] == {"@type": "Place", "name": "Banskobystrický kraj",
                                         "sameAs": "http://data.europa.eu/nuts/code/SK032"}
    assert record["license"] == "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0"
    assert record["creator"] == {"@type": "Organization", "name": "Mesto Banská Bystrica",
                                 "sameAs": "https://data.gov.sk/id/legal-subject/00313271"}
    assert record["temporalCoverage"] == "2025-06-06/.."
    assert len(record["distribution"]) == len(pkg["resources"])
    assert all(d["contentUrl"].startswith(("https://", "http://")) and d["encodingFormat"] for d in record["distribution"])
    harvested = dict(PUBLISHED, name="hki", title="Air", notes="x" * 60, extras=PUBLISHED["extras"] + [
        {"key": "temporal_end", "value": "2025-12-31"}])
    assert site.jc_schema_org_dict(harvested)["temporalCoverage"] == "2024-01-01/2025-12-31"


def test_a_description_cannot_close_the_script_element(site):
    """A dataset's words are the publisher's: `</script>` in them must not end the element."""
    hostile = "</script><script>alert(1)</script> & <!-- " + "x" * 50
    text = site.jc_schema_org(dict(PUBLISHED, name="x", title="T</script>", notes=hostile))
    assert "<" not in text and ">" not in text and "&" not in text
    assert json.loads(text)["description"] == hostile


def test_the_dataset_page_carries_the_record_in_its_head():
    page = (THEME / "templates__package__read_base.html").read_text()
    head = re.search(r'{% block head_extras %}\s*{{- super\(\) }}\s*<script type="application/ld\+json">'
                     r'{{ h.jc_schema_org\(pkg\) \| safe }}</script>(.*?){% endblock %}', page, re.S)
    assert head
    # T-5203: and one alternate link per feed output and format, nothing else.
    alternates = re.sub(r"{#.*?#}", "", head.group(1), flags=re.S).split()
    assert " ".join(alternates) == (
        '{% for feed in h.jc_subscriptions(pkg).news %} <link rel="alternate" type="{{ feed.mimetype }}" '
        'title="{{ feed.name }}" href="{{ feed.url }}"> {% endfor %}')
    assert check_schema_org.check_page(
        '<head><script type="application/ld+json">{"@type": "Dataset"}</script></head>') != []
    assert check_schema_org.check_page("<head></head>") == ["0 schema.org Dataset records, expected 1"]


def test_the_sitemap_lists_each_public_dataset_and_organization_and_nothing_hidden(site, monkeypatch):
    """Security: the search is anonymous, without private or draft datasets, and a hidden one is
    left out even when a sysadmin asks (the hidden filter alone follows the caller)."""
    monkeypatch.setattr(site, "HIDDEN", frozenset({"bb-dotacie"}))
    calls = []

    def search(context, data_dict):
        calls.append((context, data_dict))
        rows = DEV_DATASETS[data_dict["start"]:data_dict["start"] + 40]
        # pysolr hands a date back as a datetime.
        return {"count": len(DEV_DATASETS), "results": [
            {"name": p["name"], "metadata_modified": datetime.datetime.fromisoformat(p["metadata_modified"])} for p in rows]}

    entries = site.jc_sitemap_entries(search, lambda context, data_dict: ["banskabystrica", "bbsk"])
    assert all(c[0] == {"user": "", "ignore_auth": False} and c[1]["include_private"] is False
               and c[1]["include_drafts"] is False for c in calls)
    assert len(calls) == 3
    datasets = [loc for loc, _ in entries if "/dataset/" in loc]
    assert datasets == ["https://opendata.dev.example/dataset/" + p["name"] for p in DEV_DATASETS if p["name"] != "bb-dotacie"]
    assert entries[-2:] == [("https://opendata.dev.example/organization/banskabystrica", ""),
                            ("https://opendata.dev.example/organization/bbsk", "")]
    xml = site.jc_sitemap_xml(entries + [("https://x.example/dataset/a&b", "not a date")])
    root = ElementTree.fromstring(xml)
    locs = [e.text for e in root.iter("{http://www.sitemaps.org/schemas/sitemap/0.9}loc")]
    assert locs[-1] == "https://x.example/dataset/a&b" and len(locs) == len(entries) + 1
    assert "<lastmod>2026-" in xml and "not a date" not in xml
    assert calls[0][1]["fl"] == ["name", "metadata_modified"]


def test_the_sitemap_stops_at_the_protocol_s_limit(site, monkeypatch):
    monkeypatch.setattr(site, "SITEMAP_MAX", 5)
    entries = site.jc_sitemap_entries(
        lambda c, d: {"count": 10**6, "results": [{"name": f"d{d['start'] + i}"} for i in range(1000)]},
        lambda c, d: ["org"])
    assert len(entries) == 5


def test_robots_names_the_sitemap_and_a_noindex_demo_closes_the_site(site, monkeypatch):
    robots = site.jc_robots_txt()
    assert "Sitemap: https://opendata.dev.example/sitemap.xml\n" in robots and "Disallow: /api/" in robots
    assert re.findall(r"(?im)^sitemap:\s*(\S+)", robots) == ["https://opendata.dev.example/sitemap.xml"]
    monkeypatch.setitem(site.BRANDING, "noindex", True)
    assert site.jc_robots_txt() == "User-agent: *\nDisallow: /\n"


@pytest.mark.parametrize("count, folded", [(3, False), (4, True), (60, True)])
def test_about_folds_many_sources_into_one_disclosure(theme, count, folded):
    """T-5192: bbsk-podujatia listed ~60 "Original data" rows before its licence and contact. More
    than three fold into one row with a count, the rows before and after stay where they were."""
    jinja2 = pytest.importorskip("jinja2")
    theme.page.lang = "sk"
    read = (THEME / "templates__package__read.html").read_text()
    block = read[read.index('<dl class="jc-about-list">'):read.index("</dl>") + len("</dl>")]
    source = theme.jc_t("source")
    about = ([(theme.jc_t("publisher"), "BBSK", None)]
             + [(source, "Mesto %d" % i, "https://mesto%d.sk/podujatia" % i) for i in range(count)]
             + [(theme.jc_t("contact"), "joinedcontext", None)])
    html = jinja2.Environment(autoescape=True).from_string(block).render(
        about=about, h=types.SimpleNamespace(jc_t=theme.jc_t))
    assert html.count("<dt>") == (3 if folded else count + 2)
    assert html.count('<details class="jc-about-sources">') == (1 if folded else 0)
    assert html.count('href="https://mesto') == count
    if folded:
        assert "Zobraziť všetky (%d)" % count in html
        assert html.index("Vydavateľ") < html.index("jc-about-sources") < html.index("joinedcontext")


def test_viz_frames_are_sized_by_content_and_hear_the_visible_slice():
    """T-5192: a 2000 px cap cut the live data widget's map off; the frame now takes its content's
    height up to a runaway guard, and tells the frame which slice the reader sees."""
    script = (THEME / "public__jc-viz.js").read_text()
    assert "var JC_VIZ_MAX = 20000;" in script and "2000)" not in script
    assert 'type: "jc-viz-viewport"' in script and 'frame.getAttribute("data-jc-viz"))' in script


# T-5203: the dataset page offers its calendar and feeds, and the catalogue's own Atom feed is valid.
SUBSCRIBED = {
    "resources": [
        {"name": "NGSI-LD API", "url": "https://dev.example/api/endpoint/e/ngsi-ld/v1/", "mimetype": "application/json"},
        {"name": "iCalendar feed", "url": "https://dev.example/api/endpoint/e/ical/v1/events.ics", "mimetype": "text/calendar"},
        {"name": "Kultúra (Atom)", "url": "https://dev.example/api/endpoint/e/feed/v1/kultura/atom.xml", "mimetype": "application/atom+xml"},
        {"name": "Kultúra (JSON Feed)", "url": "https://dev.example/api/endpoint/e/feed/v1/kultura/feed.json", "mimetype": "application/feed+json"},
        {"name": "Planted", "url": "javascript:alert(1)", "mimetype": "application/rss+xml"},
    ]
}


def test_subscriptions_group_calendar_and_news_and_keep_https_only(theme):
    found = theme.jc_subscriptions(SUBSCRIBED)
    assert [item["name"] for item in found["calendar"]] == ["iCalendar feed"]
    assert [item["name"] for item in found["news"]] == ["Kultúra (Atom)", "Kultúra (JSON Feed)"]
    assert theme.jc_subscriptions({"resources": SUBSCRIBED["resources"][:1]}) == {"calendar": [], "news": []}


def test_the_catalogue_feeds_paging_links_are_absolute_and_followable(theme):
    site = "https://opendata.dev.example"
    assert theme._feed_href("/feeds/dataset.atom?query=page&query=2", site) == f"{site}/feeds/dataset.atom?page=2"
    assert theme._feed_href("/feeds/dataset.atom?query=page&query=5.0", site) == f"{site}/feeds/dataset.atom?page=5"
    assert theme._feed_href("/dataset/", site) == f"{site}/dataset/"
    assert theme._feed_href("https://other.example/x?page=1", site) == "https://other.example/x?page=1"


def test_the_catalogue_feed_names_its_language_author_and_each_publisher(theme):
    made = {}

    class Base:
        def __init__(self, title, link, description, **kwargs):
            made.update(title=title, link=link, **kwargs)
            made["items"] = []

        def add_item(self, **kwargs):
            made["items"].append(kwargs)

    feed = theme.jc_feed_class(Base, "https://opendata.dev.example", "sk", "Otvorené dáta")(
        "Otvorené dáta", "/dataset/", "Recently", language="en", author_name="default",
        next_page="/feeds/dataset.atom?query=page&query=2", previous_page=None,
    )
    assert made["language"] == "sk" and made["author_name"] == "Otvorené dáta"
    assert made["link"] == "https://opendata.dev.example/dataset/"
    assert made["next_page"] == "https://opendata.dev.example/feeds/dataset.atom?page=2"
    assert made["previous_page"] is None
    feed.add_item(title="Ovzdušie", link="https://opendata.dev.example/api/3/action/package_show?id=x",
                  author_name="", author_email="", jc_publisher="Mesto Banská Bystrica",
                  jc_page="https://opendata.dev.example/dataset/ovzdusie")
    feed.add_item(title="Bez vydavateľa", link="l", author_name="", author_email="")
    assert made["items"][0] == {"title": "Ovzdušie", "link": "https://opendata.dev.example/dataset/ovzdusie",
                                "author_name": "Mesto Banská Bystrica"}
    assert made["items"][1] == {"title": "Bez vydavateľa", "link": "l"}


# T-5147: the catalogue's own CSP, a nonce per request for the inline scripts and styles CKAN and
# the theme still write, nothing inline else.
@pytest.fixture
def request_g(theme, monkeypatch):
    g = types.SimpleNamespace()
    monkeypatch.setattr(theme.toolkit, "g", g, raising=False)
    return g


def _directives(policy):
    return {part.split()[0]: part.split()[1:] for part in policy.split("; ")}


def test_the_catalogue_policy_allows_scripts_and_styles_from_itself_and_the_nonce_only(theme):
    policy = theme._csp("n0nce")
    d = _directives(policy)
    assert d["script-src"] == ["'self'", "'nonce-n0nce'"] and d["style-src"] == ["'self'", "'nonce-n0nce'"]
    assert "unsafe" not in policy and "*;" not in policy and " https:;" not in policy and " data:;" not in policy.replace("img-src 'self' data:", "")
    # Frames, fetches and images: this installation's own hosts, the Endpoints, tiles, viz and the assistant.
    own = ["'self'", "https://dev.example", "https://*.dev.example"]
    assert d["connect-src"] == own and d["frame-src"] == own and d["img-src"] == ["'self'", "data:"] + own[1:]
    # The directives that fall back to nothing are named (ZAP 10055), the edge's baseline kept.
    assert d["form-action"] == ["'self'"] and d["frame-ancestors"] == ["'self'"]
    assert d["object-src"] == ["'none'"] and d["base-uri"] == ["'self'"] and d["default-src"] == ["'self'"]


def test_without_a_branded_domain_the_policy_names_no_other_host(theme, monkeypatch):
    monkeypatch.setitem(theme.BRANDING, "domain", "evil.example; script-src *")
    assert "evil" not in theme._csp("n") and _directives(theme._csp("n"))["connect-src"] == ["'self'"]
    monkeypatch.delitem(theme.BRANDING, "domain")
    assert _directives(theme._csp("n"))["frame-src"] == ["'self'"]


def test_the_nonce_is_one_per_request_and_unguessable(theme, request_g, monkeypatch):
    first = theme.jc_csp_nonce()
    assert theme.jc_csp_nonce() == first and len(first) >= 24 and re.fullmatch(r"[A-Za-z0-9_-]+", first)
    monkeypatch.setattr(theme.toolkit, "g", types.SimpleNamespace(), raising=False)
    assert theme.jc_csp_nonce() != first


class _Response:
    def __init__(self, mimetype, headers=None):
        self.mimetype = mimetype
        self.headers = dict(headers or {})


def test_every_html_answer_without_a_policy_gets_the_catalogue_s_with_the_page_s_nonce(theme, request_g):
    nonce = theme.jc_csp_nonce()  # the page's templates asked for it while rendering
    page = theme.jc_csp_header(_Response("text/html"))
    assert page.headers["Content-Security-Policy"] == theme._csp(nonce)
    # A page naming its own policy (the map page, OPS-34) keeps it; JSON and files get none here.
    own = theme.jc_csp_header(_Response("text/html", {"Content-Security-Policy": "frame-ancestors *"}))
    assert own.headers == {"Content-Security-Policy": "frame-ancestors *"}
    assert theme.jc_csp_header(_Response("application/json")).headers == {}


INLINE_SCRIPT = re.compile(r"<script(?![^>]*\bsrc=)(?![^>]*type=\"application/ld\+json\")[^>]*>")


def test_no_theme_template_writes_a_script_style_or_handler_the_policy_refuses():
    for f in THEME.glob("templates__*.html"):
        # The embed snippet a reader copies is text in a textarea, not markup of this page.
        text = re.sub(r"<textarea[^>]*>.*?</textarea>", "", f.read_text(), flags=re.S)
        for tag in INLINE_SCRIPT.findall(text) + re.findall(r"<style\b[^>]*>", text):
            assert 'nonce="{{ h.jc_csp_nonce() }}"' in tag, (f.name, tag)
        assert not re.search(r"\son[a-z]+\s*=", text), f.name
        assert not re.search(r"\sstyle\s*=", text), f.name
        assert "javascript:" not in text, f.name


def test_the_core_templates_with_inline_script_or_handlers_are_overridden():
    """CKAN 2.11.7's inline scripts (dataviewer, text view, table view) and onclick attributes
    (resource upload field, Refit) each have a theme copy that the test above holds."""
    for name in ("dataviewer__base", "text_view", "datatables__datatables_view", "package__snippets__resource_upload_field", "image_view"):
        assert (THEME / ("templates__%s.html" % name)).exists(), name
    view = (THEME / "templates__datatables__datatables_view.html").read_text()
    assert view.count("{%- block page -%}") == 1 and 'id="refit-button"' in view and '<div id="dtv-resource-info" hidden>' in view
    for name in ("templates__base.html", "templates__datatables__datatables_view.html"):
        assert "/jc-forms.js" in (THEME / name).read_text(), name


def test_the_table_view_copy_keeps_ckan_2_11_7_s_guard():
    """CKAN 2.11.7's table view (T-6643) draws the table only when the DataStore names the
    resource's fields and says the view is unavailable otherwise; the theme's page block replaces
    CKAN's whole, so the copy carries the guard itself (the helper exists in 2.11.6 too)."""
    view = (THEME / "templates__datatables__datatables_view.html").read_text()
    page = view.split("{%- block page -%}", 1)[1]
    assert page.lstrip().startswith("{% if h.resource_view_get_fields(resource) %}")
    assert page.index("{% else %}") > page.index('<div id="dtv-resource-info" hidden>')
    assert "This resource view is not available at the moment." in page
    assert page.rstrip().endswith("{% endif %}\n{%- endblock -%}")


def test_the_theme_stylesheet_holds_what_style_attributes_and_htmx_wrote_inline():
    css = (THEME / "public__jc-theme.css").read_text()
    for rows in range(1, 20):
        assert '.jc-grid[data-jc-rows="%d"] { --jc-rows: %d; }' % (rows, rows) in css
    assert ".htmx-request .htmx-indicator" in css and ".jc-image-view {" in css
    assert """<meta name="htmx-config" content='{"includeIndicatorStyles": false}'>""" in (THEME / "templates__base.html").read_text()


def _forms_click(button_attrs, elements):
    """Runs jc-forms.js on one click in node, with the few DOM calls it makes."""
    script = (THEME / "public__jc-forms.js").read_text()
    code = """
const log = [];
const els = %s;
for (const [id, el] of Object.entries(els)) {
  el.id = id;
  el.click = () => log.push("click " + id);
  el.focus = () => log.push("focus " + id);
}
class Element {}
globalThis.Element = Element;
let listener;
globalThis.window = {};
globalThis.document = { addEventListener: (t, fn) => { listener = fn; }, getElementById: (id) => els[id] ?? null };
%s
const button = Object.assign(new Element(), { dataset: %s, closest: (sel) => sel === "[data-jc-check]" ? button : null });
listener({ target: button, preventDefault() {} });
console.log(JSON.stringify({ log, els }));
""" % (json.dumps(elements), script, json.dumps(button_attrs))
    done = subprocess.run(["node", "--input-type=module", "-e", code], capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stderr
    return json.loads(done.stdout)


@pytest.mark.skipif(not shutil.which("node"), reason="node is not installed")
def test_the_resource_form_buttons_do_what_their_onclick_did():
    # Upload: tick the upload radio and open the file picker.
    out = _forms_click({"jcCheck": "resource-url-upload", "jcClick": "field-resource-upload"},
                       {"resource-url-upload": {"checked": False}, "field-resource-upload": {"value": ""}})
    assert out["els"]["resource-url-upload"]["checked"] is True and out["log"] == ["click field-resource-upload"]
    # Remove: back to no data, the field emptied, focus on the first button.
    out = _forms_click({"jcCheck": "resource-url-none", "jcClear": "field-resource-url", "jcFocus": "resource-upload-button"},
                       {"resource-url-none": {"checked": False}, "field-resource-url": {"value": "https://x/a.csv"},
                        "resource-upload-button": {}})
    assert out["els"]["resource-url-none"]["checked"] is True and out["els"]["field-resource-url"]["value"] == ""
    assert out["log"] == ["focus resource-upload-button"]
    # A button naming an element the page lacks does nothing else.
    assert _forms_click({"jcCheck": "nope", "jcFocus": "gone"}, {})["log"] == []


def test_the_upload_field_copy_keeps_every_button_of_ckan_s():
    text = (THEME / "templates__package__snippets__resource_upload_field.html").read_text()
    assert 'data-jc-check="resource-url-upload" data-jc-click="field-resource-upload"' in text
    assert 'data-jc-check="resource-url-link" data-jc-focus="field-resource-url"' in text
    assert 'data-jc-check="field-clear-upload" data-jc-focus="field-resource-upload"' in text
    assert "{{ remove_button('field-resource-upload') }}" in text and "{{ remove_button('field-resource-url') }}" in text
