#![forbid(unsafe_code)]
//! Port of `ours/ckan-theme/plugin.py` template helpers (`jc_about`, `jc_live`,
//! `jc_resource_sections`, `jc_resource_label`, `jc_examples`, `jc_usage`, `jc_preview`,
//! `jc_visualizations`, `jc_explorer`, `jc_map`, `jc_subscriptions`, `jc_schema_org_dict`,
//! `jc_developer_page`, `jc_mcp_install`, `jc_assistants`, `jc_assistant_guide`, `jc_page`,
//! `jc_sitemap_xml`, `jc_robots_txt`, `jc_plain_glyphs`, NUTS 3 lookup) for `dataroom-web` (D1-D7).

use crate::i18n::{format_date_localized, jc_rows, jc_t, normalize_lang_code};
use chrono::{DateTime, Datelike, Duration, Utc};
use dataroom_core::vocab::find_licence;
use dataroom_core::{
    localized_text, resolve_publisher_branding, BrandingBlock, Catalogue, Dataset, Distribution,
    Publisher, PublisherBranding, RecordState, ResourceView, Visibility,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const NUTS_JSON: &str = include_str!("../../../ours/ckan-theme/nuts.json");

/// Looks up a NUTS code (e.g. `SK032`, `CZ010`) in `nuts.json` in `lang` (`_nuts_names` in `plugin.py`, D2, D3).
pub fn nuts_label(uri_or_code: &str, lang: &str) -> String {
    let code = uri_or_code
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(uri_or_code);
    let is_nuts = (2..=5).contains(&code.len())
        && code.as_bytes()[..2].iter().all(u8::is_ascii_uppercase)
        && code.as_bytes()[2..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if !is_nuts {
        return uri_or_code.to_owned();
    }
    let text = format!("NUTS {code}");
    let Ok(parsed) = serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>(NUTS_JSON)
    else {
        return text;
    };
    if let Some(names) = parsed.get(code) {
        let l = normalize_lang_code(lang);
        if let Some(name) = names.get(&l).or_else(|| names.get("en")) {
            return format!("{name} ({text})");
        }
    }
    text
}

/// Replaces Word Symbol-font private-use bullets (`\u{f0b7}` etc.) with Unicode bullets and drops
/// any other private-use code point (`jc_plain_glyphs` in `plugin.py` line 1882).
pub fn jc_plain_glyphs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{f0b7}' => out.push('•'),
            '\u{f0a7}' => out.push('▪'),
            '\u{f0a8}' => out.push('◦'),
            '\u{f0d8}' => out.push('›'),
            '\u{f0fc}' => out.push('✓'),
            '\u{f076}' => out.push('❖'),
            c if ('\u{e000}'..='\u{f8ff}').contains(&c) => {}
            other => out.push(other),
        }
    }
    out
}

/// Breakable piece of a URL or shell command (`jc_pieces` in `plugin.py` line 868).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakablePiece {
    pub text: String,
    pub space: bool,
}

/// Splits a command or URL into pieces that only break after `/`, `?` or `&`, or on whitespace
/// (`jc_pieces` in `plugin.py`, D3, D4).
pub fn jc_pieces(text: &str) -> Vec<BreakablePiece> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut in_space: Option<bool> = None;

    for ch in text.chars() {
        let is_sp = ch.is_whitespace();
        if in_space == Some(true) && !is_sp {
            pieces.push(BreakablePiece {
                text: std::mem::take(&mut current),
                space: true,
            });
        } else if in_space == Some(false) && is_sp {
            pieces.extend(split_non_space_token(&std::mem::take(&mut current)));
        }
        in_space = Some(is_sp);
        current.push(ch);
    }
    if !current.is_empty() {
        if in_space == Some(true) {
            pieces.push(BreakablePiece {
                text: current,
                space: true,
            });
        } else {
            pieces.extend(split_non_space_token(&current));
        }
    }
    pieces
}

fn split_non_space_token(token: &str) -> Vec<BreakablePiece> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = token.chars().peekable();
    while let Some(c) = chars.next() {
        buf.push(c);
        if c == '/' {
            while chars.peek() == Some(&'/') {
                buf.push(chars.next().unwrap_or('/'));
            }
            out.push(BreakablePiece {
                text: std::mem::take(&mut buf),
                space: false,
            });
        } else if c == '?' || c == '&' {
            out.push(BreakablePiece {
                text: std::mem::take(&mut buf),
                space: false,
            });
        }
    }
    if !buf.is_empty() {
        out.push(BreakablePiece {
            text: buf,
            space: false,
        });
    }
    out
}

/// Renders `jc_pieces` as HTML `<span class="jc-nb">...</span><wbr>` matching `snippets/jc_breakable.html`.
pub fn render_breakable_html(text: &str) -> String {
    let mut out = String::new();
    for piece in jc_pieces(text) {
        if piece.space {
            out.push_str(&html_escape(&piece.text));
        } else {
            out.push_str("<span class=\"jc-nb\">");
            out.push_str(&html_escape(&piece.text));
            out.push_str("</span><wbr>");
        }
    }
    out
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Determines the kind of an Endpoint distribution from its path under `/api/endpoint/{slug}/`
/// (`jc_kind` in `plugin.py` line 676, D3, D4).
pub fn jc_kind(dist: &Distribution) -> Option<&'static str> {
    let url = dist.access_url.as_str();
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "ws" | "wss") || parsed.host_str().is_none() {
        return None;
    }
    let path = parsed.path();
    let after_ep = path.strip_prefix("/api/endpoint/")?;
    let (_, rest) = after_ep.split_once('/')?;
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return Some("DCAT");
    }
    for (prefix, kind) in [
        ("ngsi-ld/", "NGSI-LD"),
        ("ngsi-ld", "NGSI-LD"),
        ("odata/", "ODATA"),
        ("odata", "ODATA"),
        ("ogc/", "OGCFEAT"),
        ("ogc", "OGCFEAT"),
        ("sta/", "STA"),
        ("sta", "STA"),
        ("schema/", "MODEL"),
        ("schema", "MODEL"),
    ] {
        if rest.starts_with(prefix) {
            return Some(kind);
        }
    }
    match rest {
        "mcp" => Some("MCP"),
        "ws" => Some("WEBSOCKET"),
        "file.csv" => Some("CSV"),
        "file.json" => Some("JSON"),
        "file.geojson" => Some("GEOJSON"),
        "file.xlsx" => Some("XLSX"),
        "file.parquet" => Some("PARQUET"),
        _ => None,
    }
}

/// Standard format chips shown on dataset cards and search results (`jc_format_chips` / `DATA_FORMATS`
/// in `plugin.py` lines 1919-1970, D1, D2).
pub const DATA_FORMATS_ORDER: [(&str, &str); 17] = [
    ("NGSI-LD", "NGSI-LD"),
    ("WEBSOCKET", "WebSocket"),
    ("OGCFEAT", "OGC API Features"),
    ("ODATA", "OData"),
    ("STA", "SensorThings API"),
    ("GEOJSON", "GeoJSON"),
    ("CSV", "CSV"),
    ("JSON", "JSON"),
    ("XLSX", "Excel"),
    ("XLS", "XLS"),
    ("XML", "XML"),
    ("PARQUET", "Parquet"),
    ("ICS", "iCalendar"),
    ("RSS", "RSS"),
    ("ATOM", "Atom"),
    ("JSONFEED", "JSON Feed"),
    ("MCP", "MCP"),
];

pub fn data_format_label(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();
    for (k, label) in DATA_FORMATS_ORDER {
        if k == upper {
            return label.to_owned();
        }
    }
    code.to_owned()
}

/// Computes the served format codes of a dataset in `DATA_FORMATS` order (`jc_data_formats` in `plugin.py`).
pub fn jc_data_formats(ds: &Dataset) -> Vec<&'static str> {
    let mut found = BTreeSet::new();
    for dist in &ds.distributions {
        if resource_section_key(dist, ds) == "schema" || jc_is_source(dist, ds) {
            continue;
        }
        if let Some(kind) = jc_kind(dist) {
            found.insert(kind.to_owned());
            continue;
        }
        let mime = dist.media_type.as_deref().unwrap_or("").trim().to_ascii_lowercase();
        let mapped_mime = match mime.as_str() {
            "text/calendar" => Some("ICS"),
            "application/atom+xml" => Some("ATOM"),
            "application/rss+xml" => Some("RSS"),
            "application/feed+json" => Some("JSONFEED"),
            _ => None,
        };
        if let Some(m) = mapped_mime {
            found.insert(m.to_owned());
        } else if let Some(ref f) = dist.format {
            found.insert(f.trim().to_ascii_uppercase());
        }
    }
    DATA_FORMATS_ORDER
        .iter()
        .filter(|(code, _)| found.contains(*code))
        .map(|(code, _)| *code)
        .collect()
}

/// Returns the human-readable format chips for a dataset (`jc_format_chips` in `plugin.py`).
pub fn jc_format_chips(ds: &Dataset) -> Vec<String> {
    let mut out = Vec::new();
    for code in jc_data_formats(ds) {
        if let Some((_, label)) = DATA_FORMATS_ORDER.iter().find(|(c, _)| *c == code) {
            out.push((*label).to_owned());
        }
    }
    out
}

/// Checks if a distribution is the harvest's link to the publisher's own page (`jc_is_source` in `plugin.py`).
pub fn jc_is_source(dist: &Distribution, ds: &Dataset) -> bool {
    let ds_title = ds.title.values().next().map(|s| s.trim()).unwrap_or("");
    let dist_title = dist.title.values().next().map(|s| s.trim()).unwrap_or("");
    !ds_title.is_empty()
        && ds_title == dist_title
        && jc_kind(dist).is_none()
        && dist.format.as_deref().unwrap_or("").is_empty()
}

const API_FORMATS: [&str; 7] = ["NGSI-LD", "WEBSOCKET", "MCP", "OGCFEAT", "ODATA", "STA", "DCAT"];
const DATA_ORDER: [&str; 6] = ["CSV", "GEOJSON", "JSON", "XLSX", "PARQUET", "ZIP"];

pub fn resource_section_key(dist: &Distribution, ds: &Dataset) -> &'static str {
    if jc_is_source(dist, ds) {
        return "source";
    }
    let path = dist.access_url.split('?').next().unwrap_or("");
    let kind = jc_kind(dist);
    if kind == Some("MODEL") || path.contains("/schema/") {
        return "schema";
    }
    if let Some(k) = kind {
        return if API_FORMATS.contains(&k) {
            "api"
        } else {
            "data"
        };
    }
    let fmt = dist.format.as_deref().unwrap_or("").trim().to_ascii_uppercase();
    if API_FORMATS.contains(&fmt.as_str()) {
        "api"
    } else {
        "data"
    }
}

/// Ordering key for distributions matching `_order` in `plugin.py` line 708 (D3).
pub fn resource_sort_key(dist: &Distribution) -> (usize, usize, String, String) {
    let kind = jc_kind(dist);
    let name = match kind {
        Some(k) if k != "MODEL" => k.to_owned(),
        _ => dist
            .format
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase(),
    };
    let known = DATA_ORDER
        .iter()
        .position(|c| *c == name)
        .unwrap_or(DATA_ORDER.len());
    let api = API_FORMATS
        .iter()
        .position(|c| *c == name)
        .unwrap_or(API_FORMATS.len());
    let title = dist.title.values().next().cloned().unwrap_or_default();
    (known, api, name, title)
}

/// Display chip label for a resource (`KIND_LABELS` / `jc_format` in `plugin.py`).
pub fn jc_resource_format_chip(dist: &Distribution) -> String {
    match jc_kind(dist) {
        Some("DCAT") => "DCAT-AP".into(),
        Some("NGSI-LD") => "NGSI-LD".into(),
        Some("WEBSOCKET") => "WebSocket".into(),
        Some("MCP") => "MCP".into(),
        Some("OGCFEAT") => "OGC API".into(),
        Some("ODATA") => "OData".into(),
        Some("STA") => "SensorThings".into(),
        Some("CSV") => "CSV".into(),
        Some("JSON") => "JSON".into(),
        Some("GEOJSON") => "GeoJSON".into(),
        Some("XLSX") => "Excel".into(),
        Some("PARQUET") => "Parquet".into(),
        _ => dist
            .format
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("—")
            .to_owned(),
    }
}

/// Human-readable resource label (`jc_resource_label` in `plugin.py` line 798, D3, D4).
pub fn jc_resource_label(dist: &Distribution, ds: &Dataset, lang: &str, default_lang: &str) -> String {
    let raw_name = localized_text(&dist.title, lang, default_lang)
        .unwrap_or("")
        .trim();
    let rest = raw_name
        .strip_prefix(&format!("{} ", ds.name))
        .map(str::trim)
        .unwrap_or(raw_name);

    let file_tail = dist
        .access_url
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or(rest);

    let model_key = if jc_kind(dist) == Some("MODEL") || dist.access_url.contains("/schema/") {
        file_tail
    } else {
        rest
    };

    let code = normalize_lang_code(lang);
    let model_word = match (model_key, code.as_str()) {
        ("context.jsonld", "sk" | "cs") => Some("Kontext JSON-LD"),
        ("context.jsonld", "de") => Some("JSON-LD-Kontext"),
        ("context.jsonld", "hu") => Some("JSON-LD kontextus"),
        ("context.jsonld", _) => Some("JSON-LD context"),
        ("model.linkml.yaml", _) => Some("LinkML"),
        ("model.md", "sk") => Some("Popis slovami"),
        ("model.md", "cs") => Some("Popis slovy"),
        ("model.md", "de") => Some("In Worten beschrieben"),
        ("model.md", "hu") => Some("Szöveges leírás"),
        ("model.md", _) => Some("Described in words"),
        ("model.owl.ttl", _) => Some("OWL"),
        ("model.rdf.ttl", _) => Some("RDF"),
        ("model.schema.json", _) => Some("JSON Schema"),
        ("index.json", "sk") => Some("Zoznam súborov"),
        ("index.json", "cs") => Some("Seznam souborů"),
        ("index.json", "de") => Some("Liste der Dateien"),
        ("index.json", "hu") => Some("A fájlok listája"),
        ("index.json", _) => Some("List of its files"),
        ("model.shacl.ttl", "sk") => Some("Pravidlá kontroly"),
        ("model.shacl.ttl", "cs") => Some("Pravidla kontroly"),
        ("model.shacl.ttl", "de") => Some("Prüfregeln"),
        ("model.shacl.ttl", "hu") => Some("Ellenőrzési szabályok"),
        ("model.shacl.ttl", _) => Some("Validation rules"),
        _ => None,
    };
    if let Some(w) = model_word {
        return w.to_owned();
    }

    match jc_kind(dist) {
        Some("CSV") => return jc_t("records_csv", lang),
        Some("JSON") => return jc_t("records_json", lang),
        Some("GEOJSON") => return jc_t("records_geojson", lang),
        Some("XLSX") => return jc_t("records_xlsx", lang),
        Some("PARQUET") => return jc_t("records_parquet", lang),
        Some("WEBSOCKET") => return jc_t("live_changes", lang),
        Some("DCAT") => return "DCAT-AP record".into(),
        Some("NGSI-LD") => return "NGSI-LD API".into(),
        Some("MCP") => return "Model Context Protocol".into(),
        Some("OGCFEAT") => return "OGC API - Features".into(),
        Some("ODATA") => return "OData v4".into(),
        Some("STA") => return "SensorThings API".into(),
        _ => {}
    }

    if jc_is_source(dist, ds) {
        return jc_t("at_publisher", lang);
    }
    if !rest.is_empty() {
        rest.to_owned()
    } else if !raw_name.is_empty() {
        raw_name.to_owned()
    } else {
        dist.format
            .clone()
            .unwrap_or_else(|| dist.id.clone())
    }
}

/// One-line localized description of what a resource is (`jc_what` / `FORMATS` in `plugin.py`).
pub fn jc_what(dist: &Distribution, lang: &str) -> String {
    let code = normalize_lang_code(lang);
    let key = jc_kind(dist)
        .map(str::to_owned)
        .unwrap_or_else(|| dist.format.as_deref().unwrap_or("").trim().to_ascii_uppercase());
    let base = match (key.as_str(), code.as_str()) {
        ("NGSI-LD", "sk") => "Živé entity cez API",
        ("NGSI-LD", "cs") => "Živé entity přes API",
        ("NGSI-LD", "de") => "Live-Entitäten über die API",
        ("NGSI-LD", "hu") => "Élő entitások az API-n keresztül",
        ("NGSI-LD", _) => "Live entities through the API",
        ("JSON-LD", "sk") => "Prepojené dáta s kontextom",
        ("JSON-LD", "cs") => "Propojená data s kontextem",
        ("JSON-LD", "de") => "Verknüpfte Daten mit Kontext",
        ("JSON-LD", "hu") => "Kapcsolt adatok a kontextusukkal",
        ("JSON-LD", _) => "Linked data with its context",
        ("GEOJSON", "sk") => "Body a tvary pre mapu",
        ("GEOJSON", "cs") => "Body a tvary pro mapu",
        ("GEOJSON", "de") => "Punkte und Formen für eine Karte",
        ("GEOJSON", "hu") => "Pontok és alakzatok térképhez",
        ("GEOJSON", _) => "Points and shapes for a map",
        ("CSV", "sk") => "Tabuľka pre tabuľkový procesor",
        ("CSV", "cs") => "Tabulka pro tabulkový procesor",
        ("CSV", "de") => "Eine Tabelle für die Tabellenkalkulation",
        ("CSV", "hu") => "Táblázat táblázatkezelőhöz",
        ("CSV", _) => "A table for a spreadsheet",
        ("JSON", "sk") => "Strojovo čitateľný JSON",
        ("JSON", "cs") => "Strojově čitelný JSON",
        ("JSON", "de") => "Maschinenlesbares JSON",
        ("JSON", "hu") => "Géppel olvasható JSON",
        ("JSON", _) => "Machine-readable JSON",
        ("XLSX", "sk") => "Tabuľkový súbor",
        ("XLSX", "cs") => "Tabulkový soubor",
        ("XLSX", "de") => "Eine Tabellendatei",
        ("XLSX", "hu") => "Táblázatfájl",
        ("XLSX", _) => "A spreadsheet",
        ("PARQUET", "sk") => "Typované stĺpce pre analýzu dát (Apache Parquet)",
        ("PARQUET", "cs") => "Typované sloupce pro analýzu dat (Apache Parquet)",
        ("PARQUET", "de") => "Typisierte Spalten für die Datenanalyse (Apache Parquet)",
        ("PARQUET", "hu") => "Típusos oszlopok adatelemzéshez (Apache Parquet)",
        ("PARQUET", _) => "Typed columns for data analysis (Apache Parquet)",
        ("ZIP", "sk") => "Všetko v jednom súbore",
        ("ZIP", "cs") => "Vše v jednom souboru",
        ("ZIP", "de") => "Alles in einem Download",
        ("ZIP", "hu") => "Minden egy letöltésben",
        ("ZIP", _) => "Everything in one download",
        ("WEBSOCKET", "sk") => "Zmeny naživo, hneď ako nastanú (WebSocket)",
        ("WEBSOCKET", "cs") => "Změny živě, hned jak nastanou (WebSocket)",
        ("WEBSOCKET", "de") => "Änderungen live, sobald sie geschehen (WebSocket)",
        ("WEBSOCKET", "hu") => "Változások valós időben (WebSocket)",
        ("WEBSOCKET", _) => "Live updates as they happen (WebSocket)",
        ("OGCFEAT", "sk") => "Mapové vrstvy pre GIS (OGC API - Features)",
        ("OGCFEAT", "cs") => "Mapové vrstvy pro GIS (OGC API - Features)",
        ("OGCFEAT", "de") => "Kartenebenen für GIS (OGC API - Features)",
        ("OGCFEAT", "hu") => "Térképrétegek GIS-hez (OGC API - Features)",
        ("OGCFEAT", _) => "Map layers for GIS (OGC API - Features)",
        ("ODATA", "sk") => "Pre Excel a Power BI (OData)",
        ("ODATA", "cs") => "Pro Excel a Power BI (OData)",
        ("ODATA", "de") => "Für Excel und Power BI (OData)",
        ("ODATA", "hu") => "Excelhez és Power BI-hoz (OData)",
        ("ODATA", _) => "For Excel and Power BI (OData)",
        ("STA", "sk") => "Senzory a merania (SensorThings API)",
        ("STA", "cs") => "Senzory a měření (SensorThings API)",
        ("STA", "de") => "Sensoren und Messungen (SensorThings API)",
        ("STA", "hu") => "Érzékelők és mérések (SensorThings API)",
        ("STA", _) => "Sensors and observations (SensorThings API)",
        ("MCP", "sk") => "Pre AI asistentov (Model Context Protocol)",
        ("MCP", "cs") => "Pro AI asistenty (Model Context Protocol)",
        ("MCP", "de") => "Für KI-Assistenten (Model Context Protocol)",
        ("MCP", "hu") => "AI-asszisztenseknek (Model Context Protocol)",
        ("MCP", _) => "For AI assistants (Model Context Protocol)",
        ("LINKML", "sk") => "Dátový model",
        ("LINKML", "cs") => "Datový model",
        ("LINKML", "de") => "Das Datenmodell",
        ("LINKML", "hu") => "Az adatmodell",
        ("LINKML", _) => "The data model",
        ("SHACL", "sk") => "Pravidlá kontroly dátového modelu",
        ("SHACL", "cs") => "Pravidla kontroly datového modelu",
        ("SHACL", "de") => "Prüfregeln des Datenmodells",
        ("SHACL", "hu") => "Az adatmodell ellenőrzési szabályai",
        ("SHACL", _) => "Validation rules of the data model",
        _ => "",
    };
    if base.is_empty() {
        String::new()
    } else if base.ends_with('.') || base.ends_with('…') {
        base.to_owned()
    } else {
        format!("{base}.")
    }
}

/// View model for one resource row inside `jc_resource_sections` (D3, D4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceItemView {
    pub id: String,
    pub label: String,
    pub format: String,
    pub format_lower: String,
    pub what: String,
    pub url: String,
    pub page_url: String,
    pub action_label: String,
    pub is_api: bool,
}

/// View model for one section of resources (`data`, `api`, `schema`, `source`, D3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSectionView {
    pub key: String,
    pub title: String,
    pub lead: String,
    pub items: Vec<ResourceItemView>,
}

/// Groups and sorts a dataset's distributions into `data`, `api`, `schema`, and `source` sections
/// (`jc_resource_sections` in `plugin.py` line 769, D3).
pub fn jc_resource_sections(
    ds: &Dataset,
    lang: &str,
    default_lang: &str,
    lang_prefix: &str,
) -> Vec<ResourceSectionView> {
    let mut sorted = ds.distributions.clone();
    sorted.sort_by_key(resource_sort_key);

    let mut buckets: BTreeMap<&'static str, Vec<ResourceItemView>> = BTreeMap::new();
    for dist in sorted {
        let sec = resource_section_key(&dist, ds);
        let fmt = jc_resource_format_chip(&dist);
        let label = jc_resource_label(&dist, ds, lang, default_lang);
        let what = jc_what(&dist, lang);
        let url = dist
            .download_url
            .as_ref()
            .unwrap_or(&dist.access_url)
            .clone();
        let page_url = format!("{lang_prefix}/dataset/{}/resource/{}", ds.name, dist.id);
        let action_label = if matches!(sec, "api" | "source") {
            jc_t("open", lang)
        } else {
            jc_t("download", lang)
        };
        buckets.entry(sec).or_default().push(ResourceItemView {
            id: dist.id.clone(),
            label,
            format_lower: fmt.to_ascii_lowercase(),
            format: fmt,
            what,
            url,
            page_url,
            action_label,
            is_api: sec == "api",
        });
    }

    let mut out = Vec::new();
    for key in ["data", "api", "schema", "source"] {
        if let Some(items) = buckets.remove(key) {
            if !items.is_empty() {
                out.push(ResourceSectionView {
                    key: key.to_owned(),
                    title: jc_t(&format!("section_{key}"), lang),
                    lead: jc_t(&format!("section_{key}_lead"), lang),
                    items,
                });
            }
        }
    }
    out
}

/// One row of the "About this dataset" `<dl class="jc-about-list">` (`jc_about` in `plugin.py`, D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AboutRow {
    pub label: String,
    pub text: String,
    pub link: Option<String>,
}

/// Builds the "About this dataset" rows (`jc_about` in `plugin.py` lines 570-632, D3).
pub fn jc_about(
    ds: &Dataset,
    publisher: Option<&Publisher>,
    lang: &str,
    default_lang: &str,
) -> Vec<AboutRow> {
    let mut rows = Vec::new();
    let pub_name = ds
        .extras
        .get("publisher_name")
        .cloned()
        .or_else(|| {
            publisher.and_then(|p| localized_text(&p.title, lang, default_lang).map(str::to_owned))
        })
        .or_else(|| publisher.map(|p| p.name.clone()))
        .unwrap_or_else(|| ds.publisher_id.clone());

    if !pub_name.is_empty() {
        let link = ds
            .extras
            .get("publisher_uri")
            .cloned()
            .or_else(|| publisher.map(|p| p.uri.clone()))
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        rows.push(AboutRow {
            label: jc_t("publisher", lang),
            text: pub_name,
            link,
        });
    }

    let lic_entry = ds
        .licence_id
        .as_deref()
        .and_then(find_licence)
        .or_else(|| ds.licence_url.as_deref().and_then(find_licence));
    let lic_title = lic_entry
        .as_ref()
        .map(|l| l.title.clone())
        .or_else(|| ds.licence_id.clone());
    if let Some(lt) = lic_title.filter(|s| !s.is_empty()) {
        let link = ds
            .licence_url
            .clone()
            .or_else(|| lic_entry.map(|l| l.url))
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        rows.push(AboutRow {
            label: jc_t("licence", lang),
            text: lt,
            link,
        });
    }

    if let Some(rights) = ds.extras.get("rights").filter(|s| !s.is_empty()) {
        rows.push(AboutRow {
            label: jc_t("attribution", lang),
            text: rights.clone(),
            link: None,
        });
    }

    if let Some(raw_src) = ds.extras.get("source") {
        for (url, title) in parse_sources_extra(raw_src) {
            rows.push(AboutRow {
                label: jc_t("source", lang),
                text: title,
                link: Some(url),
            });
        }
    }

    if !ds.tags.is_empty() {
        rows.push(AboutRow {
            label: jc_t("keywords", lang),
            text: ds.tags.join(", "),
            link: None,
        });
    }

    let freq_info = jc_frequency(ds, lang);
    if let Some((ref freq_label, _)) = freq_info {
        rows.push(AboutRow {
            label: jc_t("frequency", lang),
            text: freq_label.clone(),
            link: None,
        });
    }

    rows.push(AboutRow {
        label: jc_t("created", lang),
        text: format_date_localized(ds.issued.year(), ds.issued.month(), ds.issued.day(), lang),
        link: None,
    });
    rows.push(AboutRow {
        label: jc_t("last_update", lang),
        text: format_date_localized(
            ds.modified.year(),
            ds.modified.month(),
            ds.modified.day(),
            lang,
        ),
        link: None,
    });
    if let Some((_, Some((next_dt, hourly)))) = freq_info {
        let base_date = format_date_localized(next_dt.year(), next_dt.month(), next_dt.day(), lang);
        let text = if hourly {
            format!("{base_date}{}", next_dt.format(", %H:%M (UTC)"))
        } else {
            base_date
        };
        rows.push(AboutRow {
            label: jc_t("next_update", lang),
            text,
            link: None,
        });
    }

    let spatial_val = ds
        .spatial
        .first()
        .cloned()
        .or_else(|| ds.extras.get("spatial_uri").cloned());
    if let Some(sp) = spatial_val.filter(|s| !s.is_empty()) {
        let sp_iri = if sp.starts_with("http://") || sp.starts_with("https://") {
            sp.clone()
        } else {
            format!("http://data.europa.eu/nuts/code/{sp}")
        };
        rows.push(AboutRow {
            label: jc_t("coverage", lang),
            text: nuts_label(&sp_iri, lang),
            link: Some(sp_iri),
        });
    }

    if let Some(ref temp) = ds.temporal {
        let parts: Vec<String> = [temp.start.map(|d| d.to_string()), temp.end.map(|d| d.to_string())]
            .into_iter()
            .flatten()
            .collect();
        if !parts.is_empty() {
            rows.push(AboutRow {
                label: jc_t("period", lang),
                text: parts.join(" – "),
                link: None,
            });
        }
    }

    if let Some(ref cp) = ds.contact_point {
        let cname = localized_text(&cp.name, lang, default_lang)
            .map(str::to_owned)
            .unwrap_or_else(|| cp.email.clone());
        if !cname.is_empty() {
            let link = (!cp.email.is_empty() && cp.email.contains('@')).then(|| {
                if cp.email.starts_with("mailto:") {
                    cp.email.clone()
                } else {
                    format!("mailto:{}", cp.email)
                }
            });
            rows.push(AboutRow {
                label: jc_t("contact", lang),
                text: cname,
                link,
            });
        }
    }

    if let Some(ref ident) = ds.identifier.as_ref().or_else(|| ds.extras.get("identifier")) {
        if !ident.is_empty() {
            rows.push(AboutRow {
                label: jc_t("identifier", lang),
                text: (*ident).clone(),
                link: None,
            });
        }
    }

    let std_iri = ds
        .conforms_to
        .first()
        .cloned()
        .or_else(|| ds.extras.get("conforms_to").cloned());
    if let Some(s_iri) = std_iri.filter(|s| !s.is_empty()) {
        let std_name = standard_display_name(&s_iri);
        let link = (s_iri.starts_with("http://") || s_iri.starts_with("https://")).then_some(s_iri);
        rows.push(AboutRow {
            label: jc_t("conforms_to", lang),
            text: std_name,
            link,
        });
    }

    rows
}

fn parse_sources_extra(raw: &str) -> Vec<(String, String)> {
    let items: Vec<Value> = if raw.trim_start().starts_with('[') {
        serde_json::from_str(raw).unwrap_or_default()
    } else {
        vec![Value::String(raw.to_owned())]
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Object(o) => {
                if let Some(u) = o.get("url").and_then(Value::as_str) {
                    if u.starts_with("http://") || u.starts_with("https://") {
                        let t = o
                            .get("title")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(u);
                        out.push((u.to_owned(), t.to_owned()));
                    }
                }
            }
            Value::String(u) if u.starts_with("http://") || u.starts_with("https://") => {
                out.push((u.clone(), u));
            }
            _ => {}
        }
    }
    out
}

/// Frequency label and next expected update (`jc_frequency` in `plugin.py` lines 536-552).
pub fn jc_frequency(ds: &Dataset, lang: &str) -> Option<(String, Option<(DateTime<Utc>, bool)>)> {
    let raw = ds
        .frequency
        .as_deref()
        .or_else(|| ds.extras.get("frequency").map(String::as_str))?;
    let code = raw.trim_end_matches('/').rsplit('/').next().unwrap_or(raw).to_ascii_uppercase();
    let l = normalize_lang_code(lang);

    let (en, sk, cs, de, hu, hours): (&str, &str, &str, &str, &str, Option<i64>) = match code.as_str() {
        "CONT" | "UPDATE_CONT" => ("Continuously", "Priebežne", "Průběžně", "Laufend", "Folyamatosan", None),
        "HOURLY" => ("Every hour", "Každú hodinu", "Každou hodinu", "Stündlich", "Óránként", Some(1)),
        "DAILY" => ("Every day", "Denne", "Denně", "Täglich", "Naponta", Some(24)),
        "WEEKLY" => ("Every week", "Týždenne", "Týdně", "Wöchentlich", "Hetente", Some(7 * 24)),
        "MONTHLY" => ("Every month", "Mesačne", "Měsíčně", "Monatlich", "Havonta", Some(30 * 24)),
        "QUARTERLY" => ("Every quarter", "Štvrťročne", "Čtvrtletně", "Vierteljährlich", "Negyedévente", Some(91 * 24)),
        "ANNUAL" => ("Every year", "Ročne", "Ročně", "Jährlich", "Évente", Some(365 * 24)),
        "IRREG" => ("Irregularly", "Nepravidelne", "Nepravidelně", "Unregelmäßig", "Rendszertelenül", None),
        _ => return Some((raw.to_owned(), None)),
    };
    let word = match l.as_str() {
        "sk" => sk,
        "cs" => cs,
        "de" => de,
        "hu" => hu,
        _ => en,
    };
    let next = hours.map(|h| (ds.modified + Duration::hours(h), h < 24));
    Some((word.to_owned(), next))
}

/// Standard name lookup (`STANDARDS` / `jc_standard` in `plugin.py` line 950, D3, D4).
pub fn standard_display_name(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    for (needle, label) in [
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
    ] {
        if lower.contains(needle) {
            return label.to_owned();
        }
    }
    url.to_owned()
}

pub fn jc_standard(dist: &Distribution) -> Option<(String, String)> {
    let url = dist
        .conforms_to
        .first()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| match jc_kind(dist) {
            Some("CSV") => Some("https://www.rfc-editor.org/rfc/rfc4180"),
            Some("GEOJSON") => Some("https://www.rfc-editor.org/rfc/rfc7946"),
            Some("JSON") => Some("https://www.rfc-editor.org/rfc/rfc8259"),
            Some("NGSI-LD") => Some("https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/"),
            Some("OGCFEAT") => Some("http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"),
            Some("ODATA") => Some("https://docs.oasis-open.org/odata/odata/v4.01/"),
            Some("MCP") => Some("https://modelcontextprotocol.io/specification"),
            _ => None,
        })?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    Some((url.to_owned(), standard_display_name(url)))
}

/// Dataset model link for a resource page (`jc_model` in `plugin.py` line 981, D3, D4).
pub fn jc_model(ds: &Dataset, lang: &str, default_lang: &str) -> Option<(String, String)> {
    for dist in &ds.distributions {
        if let Some((url, name)) = jc_standard(dist) {
            if name == "Smart Data Models" {
                return Some((url, "Smart Data Model".into()));
            }
        }
    }
    for dist in &ds.distributions {
        let clean = dist.access_url.split('?').next().unwrap_or("");
        if resource_section_key(dist, ds) == "schema" && clean.ends_with("schema.json") {
            return Some((
                dist.access_url.clone(),
                jc_resource_label(dist, ds, lang, default_lang),
            ));
        }
    }
    None
}

/// "More details" extras rows (`jc_more` & `jc_extra` in `plugin.py` lines 645-667, D3).
pub fn jc_more_rows(ds: &Dataset, lang: &str) -> Vec<AboutRow> {
    const HIDDEN_EXTRAS: &[&str] = &[
        "identifier",
        "language",
        "contact",
        "conforms_to",
        "status",
        "issued",
        "modified",
        "generated_by",
        "endpoint",
        "publisher_name",
        "publisher_uri",
        "contact_name",
        "contact_email",
        "frequency",
        "spatial",
        "spatial_uri",
        "temporal_start",
        "temporal_end",
        "theme",
        "license_url",
        "harvest_source",
        "rights",
        "source",
    ];
    let code = normalize_lang_code(lang);
    let mut rows = Vec::new();
    for (k, v) in &ds.extras {
        if HIDDEN_EXTRAS.contains(&k.as_str()) || v.is_empty() {
            continue;
        }
        let label = match (k.as_str(), code.as_str()) {
            ("quality_pass_rate", "sk") => "Prešlo kontrolami kvality".into(),
            ("quality_pass_rate", "cs") => "Prošlo kontrolami kvality".into(),
            ("quality_pass_rate", "de") => "Qualitätsprüfungen bestanden".into(),
            ("quality_pass_rate", "hu") => "Megfelelt a minőségi ellenőrzéseken".into(),
            ("quality_pass_rate", _) => "Passed the quality checks".into(),
            ("source_uri", "sk") => "Pôvodný záznam".into(),
            ("source_uri", "cs") => "Původní záznam".into(),
            ("source_uri", "de") => "Ursprünglicher Eintrag".into(),
            ("source_uri", "hu") => "Eredeti rekord".into(),
            ("source_uri", _) => "Original record".into(),
            ("was_generated_by", "sk") => "Vytvoril".into(),
            ("was_generated_by", "cs") => "Vytvořil".into(),
            ("was_generated_by", "de") => "Erzeugt von".into(),
            ("was_generated_by", "hu") => "Előállította".into(),
            ("was_generated_by", _) => "Produced by".into(),
            _ => {
                let mut s = k.replace('_', " ");
                if let Some(first) = s.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                s
            }
        };
        let text = if k == "quality_pass_rate" {
            if let Ok(rate) = v.parse::<f64>() {
                if (0.0..=1.0).contains(&rate) {
                    format!("{} %", (rate * 100.0).round() as i64)
                } else {
                    v.clone()
                }
            } else {
                v.clone()
            }
        } else {
            v.clone()
        };
        let link = (text.starts_with("http://") || text.starts_with("https://")).then(|| text.clone());
        rows.push(AboutRow { label, text, link });
    }
    rows
}

/// Live NGSI-LD API box on a dataset page (`jc_live` in `plugin.py` line 640, D3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveEndpointBox {
    pub url: String,
    pub url_breakable: String,
    pub example: String,
    pub example_breakable: String,
}

pub fn jc_live(ds: &Dataset, branding: &BrandingBlock) -> Option<LiveEndpointBox> {
    let ep = ds
        .endpoint_url
        .as_deref()
        .or_else(|| ds.extras.get("endpoint").map(String::as_str))?;
    if !ep.starts_with("https://") {
        return None;
    }
    let host = ep.strip_prefix("https://")?.split('/').next()?;
    if !branding.domain.is_empty()
        && host != branding.domain
        && !host.ends_with(&format!(".{}", branding.domain))
    {
        return None;
    }
    let base = if ep.ends_with('/') {
        ep.to_owned()
    } else {
        format!("{ep}/")
    };
    let example = format!("curl -H 'Accept: application/ld+json' '{base}ngsi-ld/v1/types'");
    Some(LiveEndpointBox {
        url_breakable: render_breakable_html(&base),
        url: base,
        example_breakable: render_breakable_html(&example),
        example,
    })
}

/// Live explorer widget (`jc_explorer` in `plugin.py` line 1054 + SDK DataGrid config C15, D3, D4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorerView {
    pub title: String,
    pub lead: String,
    pub src: String,
    pub origin: String,
    pub grid_config: String,
}

pub fn jc_explorer(
    ds: &Dataset,
    dist: Option<&Distribution>,
    branding: &BrandingBlock,
    lang: &str,
    odata_url: &str,
) -> Option<ExplorerView> {
    let live = jc_live(ds, branding)?;
    let domain = if branding.domain.is_empty() {
        "dev.joinedcontext.com"
    } else {
        &branding.domain
    };
    let slug = live
        .url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(&ds.name);
    let origin = format!("https://viz.{domain}");
    let code = normalize_lang_code(lang);
    let mut src = format!(
        "{origin}/explore.html#{slug}&inline&lang={code}&theme=light&dataset={}",
        ds.name
    );
    if let Some(d) = dist {
        if let Some(et) = jc_entity_type(d) {
            src.push_str(&format!("&type={et}"));
        }
    }
    let grid = crate::odata_grid_config(
        odata_url,
        "Distributions",
        "Distribution",
        &[
            ("name", &jc_t("format", lang), "text"),
            ("access_url", &jc_t("address", lang), "link"),
        ],
    );
    Some(ExplorerView {
        title: jc_t("explorer", lang),
        lead: jc_t("explorer_lead", lang),
        src,
        origin,
        grid_config: grid.to_string(),
    })
}

/// Extracts the entity type from an Endpoint resource URL or name (`jc_entity_type` in `plugin.py`).
pub fn jc_entity_type(dist: &Distribution) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(&dist.access_url) {
        for (k, v) in parsed.query_pairs() {
            if k == "type" && !v.is_empty() {
                return Some(v.into_owned());
            }
        }
        let path = parsed.path();
        if let Some(after) = path.strip_prefix("/api/endpoint/") {
            if let Some((_, rest)) = after.split_once('/') {
                for pfx in ["odata/v4/", "ogc/features/collections/", "sta/v1.1/"] {
                    if let Some(tail) = rest.strip_prefix(pfx) {
                        let seg = tail.split('/').next().unwrap_or("");
                        if !seg.is_empty() {
                            return Some(seg.to_owned());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Developer page URL (`/d/{slug}?lang={lang}` on `portal.{domain}`, `jc_developer_page` in `plugin.py`, D3).
pub fn jc_developer_page(ds: &Dataset, branding: &BrandingBlock, lang: &str) -> Option<String> {
    let live = jc_live(ds, branding)?;
    if branding.domain.is_empty() {
        return None;
    }
    let slug = live.url.trim_end_matches('/').rsplit('/').next()?;
    let code = normalize_lang_code(lang);
    Some(format!(
        "https://portal.{}/d/{slug}?lang={code}",
        branding.domain
    ))
}

/// Published chart view (`jc_visualization` in `plugin.py` line 1030, D3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub origin: String,
    pub src: String,
    pub code: String,
    pub package_link: String,
}

pub fn jc_visualizations(
    ds: &Dataset,
    views: &[ResourceView],
    branding: &BrandingBlock,
    lang: &str,
    lang_prefix: &str,
) -> Vec<ChartView> {
    if branding.domain.is_empty() {
        return Vec::new();
    }
    let origin = format!("https://viz.{}", branding.domain);
    let code_lang = normalize_lang_code(lang);
    let mut out = Vec::new();
    for v in views {
        if v.view_type != "jc_visualization" {
            continue;
        }
        let src = format!("{origin}/#{}&inline&lang={code_lang}", v.id);
        let embed = format!("{origin}/#{}&lang={code_lang}", v.id);
        let code = format!(
            "<iframe src=\"{embed}\" title=\"{}\" width=\"100%\" height=\"480\" style=\"border:0\" loading=\"lazy\" referrerpolicy=\"no-referrer\" sandbox=\"allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox\"></iframe>",
            html_escape(&v.title)
        );
        out.push(ChartView {
            id: v.id.clone(),
            title: v.title.clone(),
            description: v.description.clone(),
            origin: origin.clone(),
            src,
            code,
            package_link: format!("{lang_prefix}/dataset/{}#jc-preview-title", ds.name),
        });
    }
    out
}

/// Subscribe block items (`jc_subscriptions` in `plugin.py` line 1903, D3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionsView {
    pub calendar: Vec<(String, String, String, String)>, // (name, url, webcal_url, mimetype)
    pub news: Vec<(String, String, String)>,             // (name, url, mimetype)
}

impl SubscriptionsView {
    pub fn is_empty(&self) -> bool {
        self.calendar.is_empty() && self.news.is_empty()
    }
}

pub fn jc_subscriptions(ds: &Dataset, lang: &str, default_lang: &str) -> SubscriptionsView {
    let mut sub = SubscriptionsView::default();
    for dist in &ds.distributions {
        if !dist.access_url.starts_with("https://") {
            continue;
        }
        let mime = dist
            .media_type
            .as_deref()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let name = localized_text(&dist.title, lang, default_lang)
            .unwrap_or(&dist.access_url)
            .to_owned();
        if mime == "text/calendar"
            || dist
                .format
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case("ICS"))
        {
            let webcal = dist.access_url.replacen("https://", "webcal://", 1);
            sub.calendar.push((
                name,
                dist.access_url.clone(),
                webcal,
                "text/calendar".into(),
            ));
        } else if matches!(
            mime.as_str(),
            "application/atom+xml" | "application/rss+xml" | "application/feed+json"
        ) {
            sub.news.push((name, dist.access_url.clone(), mime));
        }
    }
    sub
}

/// Schema.org Dataset JSON-LD dictionary (`jc_schema_org_dict` in `plugin.py` line 1612, D3).
pub fn jc_schema_org_dict(
    cat: &Catalogue,
    ds: &Dataset,
    publisher: Option<&Publisher>,
    branding: &BrandingBlock,
    lang: &str,
) -> Value {
    let site = cat.base_url();
    let url = format!("{site}/dataset/{}", ds.name);
    let title = localized_text(&ds.title, lang, &cat.default_language)
        .unwrap_or(&ds.name)
        .to_owned();
    let desc: String = localized_text(&ds.description, lang, &cat.default_language)
        .unwrap_or(&title)
        .chars()
        .take(5000)
        .collect();

    let pub_name = ds
        .extras
        .get("publisher_name")
        .cloned()
        .or_else(|| {
            publisher
                .and_then(|p| localized_text(&p.title, lang, &cat.default_language).map(str::to_owned))
        })
        .unwrap_or_else(|| ds.publisher_id.clone());
    let pub_uri = ds
        .extras
        .get("publisher_uri")
        .cloned()
        .or_else(|| publisher.map(|p| p.uri.clone()));

    let mut creator = Map::new();
    creator.insert("@type".into(), json!("Organization"));
    creator.insert("name".into(), json!(pub_name));
    if let Some(u) = pub_uri.filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
        creator.insert("sameAs".into(), json!(u));
    }

    let mut record = Map::new();
    record.insert("@context".into(), json!("https://schema.org/"));
    record.insert("@type".into(), json!("Dataset"));
    record.insert("@id".into(), json!(url));
    record.insert("url".into(), json!(url));
    record.insert("name".into(), json!(title));
    record.insert("description".into(), json!(desc));
    record.insert(
        "identifier".into(),
        json!(ds.identifier.as_deref().unwrap_or(&ds.id)),
    );
    record.insert("isAccessibleForFree".into(), json!(true));
    record.insert("creator".into(), Value::Object(creator.clone()));
    record.insert("publisher".into(), Value::Object(creator));
    record.insert(
        "includedInDataCatalog".into(),
        json!({
            "@type": "DataCatalog",
            "name": branding.site_title_in(lang),
            "url": format!("{site}/"),
        }),
    );
    if !ds.tags.is_empty() {
        record.insert("keywords".into(), json!(ds.tags));
    }
    record.insert("datePublished".into(), json!(ds.issued.to_rfc3339()));
    record.insert("dateModified".into(), json!(ds.modified.to_rfc3339()));

    if let Some(ref lic) = ds.licence_url {
        if lic.starts_with("http://") || lic.starts_with("https://") {
            record.insert("license".into(), json!(lic));
        }
    }
    if let Some(sp) = ds
        .spatial
        .first()
        .or_else(|| ds.extras.get("spatial_uri"))
    {
        let sp_iri = if sp.starts_with("http://") || sp.starts_with("https://") {
            sp.clone()
        } else {
            format!("http://data.europa.eu/nuts/code/{sp}")
        };
        record.insert(
            "spatialCoverage".into(),
            json!({
                "@type": "Place",
                "name": nuts_label(&sp_iri, lang),
                "sameAs": sp_iri,
            }),
        );
    }

    let start = ds
        .temporal
        .as_ref()
        .and_then(|t| t.start.map(|d| d.to_string()))
        .unwrap_or_else(|| ds.issued.format("%Y-%m-%d").to_string());
    let end = ds
        .temporal
        .as_ref()
        .and_then(|t| t.end.map(|d| d.to_string()))
        .unwrap_or_else(|| "..".into());
    record.insert("temporalCoverage".into(), json!(format!("{start}/{end}")));

    let dists: Vec<Value> = ds
        .distributions
        .iter()
        .filter(|d| {
            (d.access_url.starts_with("http://") || d.access_url.starts_with("https://"))
                && (d.media_type.is_some() || d.format.is_some())
        })
        .map(|d| {
            let dname = localized_text(&d.title, lang, &cat.default_language)
                .or(d.format.as_deref())
                .unwrap_or(&d.access_url);
            json!({
                "@type": "DataDownload",
                "name": dname,
                "contentUrl": d.access_url,
                "encodingFormat": d.media_type.as_deref().or(d.format.as_deref()).unwrap_or(""),
            })
        })
        .collect();
    if !dists.is_empty() {
        record.insert("distribution".into(), Value::Array(dists));
    }

    Value::Object(record)
}

/// Serializes `jc_schema_org_dict` safely for an inline `<script type="application/ld+json">` tag.
pub fn jc_schema_org_script(
    cat: &Catalogue,
    ds: &Dataset,
    publisher: Option<&Publisher>,
    branding: &BrandingBlock,
    lang: &str,
) -> String {
    let val = jc_schema_org_dict(cat, ds, publisher, branding, lang);
    serde_json::to_string(&val)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// MCP install snippet (`jc_mcp_install` in `plugin.py` lines 2190-2222, D3, D6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInstallView {
    pub url: String,
    pub url_wbr: String,
    pub claude: String,
    pub claude_breakable: String,
    pub claude_copy: String,
    pub json: String,
    pub json_breakable: String,
    pub json_copy: String,
    pub vscode: String,
    pub cursor: String,
}

pub fn jc_mcp_install(branding: &BrandingBlock) -> Option<McpInstallView> {
    let domain = branding.domain.trim();
    if domain.is_empty() || !domain.contains('.') {
        return None;
    }
    let mut url = format!("https://api.{domain}/api/mcp");
    if let Some(ref own) = branding.mcp_server {
        if own.starts_with("d/") && own.len() > 2 {
            url.push('/');
            url.push_str(own);
        }
    }
    let url_wbr = url.replacen("/api", "<wbr>/api", 1).replacen("/mcp", "<wbr>/mcp", 1);
    let claude = format!("claude mcp add --transport http \\\n  joinedcontext \\\n  {url}");
    let claude_copy = format!("claude mcp add --transport http joinedcontext {url}");
    let json_display = format!(
        "{{\"mcpServers\": {{\"joinedcontext\": {{\n  \"type\": \"http\",\n  \"url\":\n  \"{url}\"\n}}}}}}"
    );
    let json_copy = serde_json::to_string_pretty(&json!({
        "mcpServers": {
            "joinedcontext": {
                "type": "http",
                "url": url,
            }
        }
    }))
    .unwrap_or_default();

    let vscode_payload = format!(
        "{{\"name\":\"joinedcontext\",\"type\":\"http\",\"url\":\"{url}\"}}"
    );
    let vscode_enc: String = url::form_urlencoded::byte_serialize(vscode_payload.as_bytes()).collect();
    let vscode = format!("vscode:mcp/install?{vscode_enc}");

    use base64::Engine as _;
    let cursor_b64 = base64::engine::general_purpose::STANDARD
        .encode(format!("{{\"url\":\"{url}\"}}").as_bytes());
    let cursor_enc: String = url::form_urlencoded::byte_serialize(cursor_b64.as_bytes()).collect();
    let cursor = format!(
        "cursor://anysphere.cursor-deeplink/mcp/install?name=joinedcontext&config={cursor_enc}"
    );

    Some(McpInstallView {
        claude_breakable: render_breakable_html(&claude),
        json_breakable: render_breakable_html(&json_display),
        url,
        url_wbr,
        claude,
        claude_copy,
        json: json_display,
        json_copy,
        vscode,
        cursor,
    })
}

/// Live preview configuration for a resource page (`jc_preview` in `plugin.py` line 889, D3, D4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePreviewConfig {
    pub url: String,
    pub kind: String,
    pub heading: String,
}

pub fn jc_preview(dist: &Distribution, branding: &BrandingBlock, lang: &str) -> Option<ResourcePreviewConfig> {
    let kind = jc_kind(dist)?;
    let url = dist.access_url.as_str();
    if !url.starts_with("https://") {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if !branding.domain.is_empty()
        && host != branding.domain
        && !host.ends_with(&format!(".{}", branding.domain))
    {
        return None;
    }
    let path = parsed.path();
    if kind == "NGSI-LD" && path.contains("/entities") {
        return Some(ResourcePreviewConfig {
            url: format!("{url}&limit=5&options=keyValues&count=true"),
            kind: "NGSI-LD".into(),
            heading: jc_t("preview_api", lang),
        });
    }
    if kind == "NGSI-LD" {
        let base = url.split('?').next().unwrap_or(url).trim_end_matches('/');
        return Some(ResourcePreviewConfig {
            url: format!("{base}/types"),
            kind: "NGSI-LD-TYPES".into(),
            heading: jc_t("preview_types", lang),
        });
    }
    if matches!(kind, "ODATA" | "STA") && jc_entity_type(dist).is_some() {
        let sep = if url.contains('?') { '&' } else { '?' };
        return Some(ResourcePreviewConfig {
            url: format!("{url}{sep}$top=5"),
            kind: kind.into(),
            heading: jc_t("preview_api", lang),
        });
    }
    if kind == "OGCFEAT" && path.trim_end_matches('/').ends_with("/items") {
        let sep = if url.contains('?') { '&' } else { '?' };
        return Some(ResourcePreviewConfig {
            url: format!("{url}{sep}limit=5"),
            kind: "OGCFEAT".into(),
            heading: jc_t("preview_api", lang),
        });
    }
    None
}

/// Usage example on a resource page (`jc_examples` in `plugin.py` line 911, D3, D4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceExample {
    pub id: String,
    pub title: String,
    pub steps: Vec<String>,
    pub code: Option<String>,
    pub code_breakable: String,
}

pub fn jc_examples(dist: &Distribution, lang: &str) -> Vec<ResourceExample> {
    let Some(kind) = jc_kind(dist) else {
        return Vec::new();
    };
    if kind == "MODEL" || dist.access_url.is_empty() {
        return Vec::new();
    }
    let url = &dist.access_url;
    let entity_type = jc_entity_type(dist).unwrap_or_default();
    let mut out = Vec::new();
    let mut add = |title: String, code: Option<String>, steps: Vec<String>| {
        let idx = out.len();
        let cb = code.as_deref().map(render_breakable_html).unwrap_or_default();
        out.push(ResourceExample {
            id: format!("jc-ex-{}-{idx}", dist.id),
            title,
            steps,
            code,
            code_breakable: cb,
        });
    };

    match kind {
        "NGSI-LD" if !url.contains("/entities") => {
            let base = url.split('?').next().unwrap_or(url).trim_end_matches('/');
            add(
                jc_t("ex_types", lang),
                Some(format!("curl -H 'Accept: application/ld+json' '{base}/types'")),
                Vec::new(),
            );
        }
        "NGSI-LD" => {
            let sep = if url.contains('?') { '&' } else { '?' };
            add(
                jc_t("ex_first", lang),
                Some(format!(
                    "curl -H 'Accept: application/ld+json' '{url}{sep}limit=5'"
                )),
                Vec::new(),
            );
        }
        "ODATA" | "STA" => {
            let sep = if url.contains('?') { '&' } else { '?' };
            add(
                jc_t("ex_first", lang),
                Some(format!("curl '{url}{sep}$top=5'")),
                Vec::new(),
            );
            if kind == "ODATA" {
                add(jc_t("ex_excel", lang), Some(url.clone()), Vec::new());
            }
        }
        "OGCFEAT" => {
            let sep = if url.contains('?') { '&' } else { '?' };
            add(
                jc_t("ex_first", lang),
                Some(format!("curl '{url}{sep}limit=5'")),
                Vec::new(),
            );
            let root = url.split("/collections/").next().unwrap_or(url);
            add(
                jc_t("ex_qgis", lang),
                Some(format!("{}/", root.trim_end_matches('/'))),
                Vec::new(),
            );
        }
        "MCP" => {
            add(
                jc_t("mcp_claude_ai", lang),
                Some(url.clone()),
                vec![jc_t("mcp_claude_ai_1", lang), jc_t("mcp_claude_ai_2", lang)],
            );
            add(
                jc_t("ex_chatgpt_title", lang),
                Some(url.clone()),
                vec![jc_t("ex_chatgpt", lang)],
            );
            add(
                jc_t("mcp_claude", lang),
                Some(format!("claude mcp add --transport http joinedcontext '{url}'")),
                Vec::new(),
            );
        }
        "WEBSOCKET" => {
            let msg = if entity_type.is_empty() {
                "{\"metadata\":{\"x-op\":\"x-subscribe\",\"x-id\":\"s1\"},\"body\":{\"type\":\"Subscription\",\"watchedAttributes\":[\"location\"]}}".to_owned()
            } else {
                format!(
                    "{{\"metadata\":{{\"x-op\":\"x-subscribe\",\"x-id\":\"s1\"}},\"body\":{{\"type\":\"Subscription\",\"entities\":[{{\"type\":\"{entity_type}\"}}]}}}}"
                )
            };
            add(
                jc_t("ex_ws", lang),
                Some(format!("websocat --protocol ngsi-ld-ws '{url}'\n{msg}")),
                Vec::new(),
            );
        }
        "CSV" | "JSON" | "GEOJSON" | "XLSX" | "PARQUET" => {
            let ext = kind.to_ascii_lowercase();
            add(
                jc_t("ex_file", lang),
                Some(format!("curl -L -o data.{ext} '{url}'")),
                Vec::new(),
            );
        }
        _ => {}
    }
    out
}

/// Map configuration for a dataset (`jc_map` in `plugin.py` lines 1152-1215, D3, D6).
pub fn jc_map_config(
    cat: &Catalogue,
    ds: &Dataset,
    resource_id: Option<&str>,
    branding: &BrandingBlock,
    lang: &str,
) -> Option<Value> {
    let ogc_dist = ds.distributions.iter().find(|d| {
        (jc_kind(d) == Some("OGCFEAT")
            || d.format
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case("OGCFEAT") || f.eq_ignore_ascii_case("GEOJSON")))
            && resource_id.is_none_or(|rid| d.id == rid)
    })?;
    let base = cat.base_url();
    let title = localized_text(&ds.title, lang, &cat.default_language)
        .unwrap_or(&ds.name)
        .to_owned();
    let base_url = ogc_dist.access_url.trim_end_matches('/');
    let layer = json!({
        "type": jc_entity_type(ogc_dist).unwrap_or_else(|| "Feature".into()),
        "items": format!("{base_url}/items"),
        "schema": format!("{base_url}/schema"),
        "tiles": format!("{base_url}/tiles/WebMercatorQuad"),
    });
    let basemap = if !branding.domain.is_empty() {
        json!({ "style": format!("https://tiles.{}/style.json", branding.domain) })
    } else {
        json!({})
    };
    let mut t = Map::new();
    for key in [
        "map_cap",
        "map_failed",
        "map_empty",
        "map_loading",
        "map_close",
        "map_panel",
        "map_length",
        "map_area",
        "map_keys",
        "map_title",
        "map_more",
    ] {
        t.insert(key.into(), json!(jc_t(key, lang)));
    }
    Some(json!({
        "layers": [layer],
        "basemap": basemap,
        "lib": "/maplibre-gl-6.14.0/maplibre-gl.mjs",
        "pmtiles": if !branding.domain.is_empty() { "/pmtiles-4.5.0.js" } else { "" },
        "enums": "/jc-enums.json",
        "lang": normalize_lang_code(lang),
        "title": title,
        "dataset": format!("{base}/dataset/{}", ds.name),
        "embed": format!("{base}/map/{}", ds.name),
        "t": t,
    }))
}

/// Static page content (`PAGES` / `jc_page` in `plugin.py` lines 2536-2676, D6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticPageView {
    pub name: String,
    pub title: String,
    pub sections: Vec<(String, Vec<String>)>,
    pub email: String,
    pub other_pages: Vec<(String, String)>,
}

pub const STATIC_PAGE_NAMES: [&str; 5] = [
    "connector",
    "privacy",
    "terms",
    "support",
    "accessibility",
];

pub fn jc_page(name: &str, branding: &BrandingBlock, lang: &str) -> Option<StaticPageView> {
    if !STATIC_PAGE_NAMES.contains(&name) {
        return None;
    }
    let code = normalize_lang_code(lang);
    let mcp = jc_mcp_install(branding);
    let mcp_url = mcp.as_ref().map(|m| m.url.as_str()).unwrap_or("https://api.localhost/api/mcp");
    let domain = if branding.domain.is_empty() {
        "localhost"
    } else {
        &branding.domain
    };

    let (title, raw_sections): (&str, Vec<(&str, Vec<String>)>) = match (name, code.as_str()) {
        ("connector", "sk") => (
            "MCP konektor",
            vec![
                (
                    "",
                    vec![format!(
                        "Pýtajte sa AI asistenta na otvorené dáta tohto katalógu: aktuálnu kvalitu ovzdušia, nemocnice v kraji, zmluvy mesta. Konektor je MCP server na adrese {mcp_url}. Netreba účet a iba číta."
                    )],
                ),
                (
                    "Čo dokáže",
                    vec![
                        "list_endpoints nájde datasety; list_types, get_type a describe_schema prečítajú ich dátové modely; query_entities a get_entity čítajú aktuálne záznamy; query_temporal a retrieve_temporal ich históriu; describe_access povie, čo smie volanie čítať.".into(),
                        "Každý nástroj iba číta. Nástroj, ktorý by zapisoval, bez prihlásenia odmietne.".into(),
                    ],
                ),
            ],
        ),
        ("connector", _) => (
            "MCP connector",
            vec![
                (
                    "",
                    vec![format!(
                        "Ask an AI assistant about this catalogue's open data: air quality now, hospitals in the region, a city's contracts. The connector is an MCP server at {mcp_url}. It needs no account and only reads."
                    )],
                ),
                (
                    "What it can do",
                    vec![
                        "list_endpoints finds the datasets; list_types, get_type and describe_schema read their data models; query_entities and get_entity read the current rows; query_temporal and retrieve_temporal read their history; describe_access says what a call may read.".into(),
                        "Every tool reads. A tool that would write is refused without a sign-in.".into(),
                    ],
                ),
            ],
        ),
        ("privacy", "sk") => (
            "Ochrana súkromia",
            vec![
                ("", vec![format!("Táto stránka sa týka MCP konektora na adrese {mcp_url}.")]),
                (
                    "Bez účtu, bez cookies",
                    vec!["Konektor odpovedá bez účtu. Nenastavuje žiadne cookie a nevedie o vás žiadny profil.".into()],
                ),
                (
                    "Dáta",
                    vec!["Konektor poskytuje iba datasety, ktoré ich vydavatelia zverejnili ako otvorené dáta.".into()],
                ),
            ],
        ),
        ("privacy", _) => (
            "Privacy",
            vec![
                ("", vec![format!("This page covers the MCP connector at {mcp_url}.")]),
                (
                    "No account, no cookie",
                    vec!["The connector answers without an account. It sets no cookie and keeps no profile of you.".into()],
                ),
                (
                    "The data",
                    vec!["The connector serves only datasets their publishers released as open data.".into()],
                ),
            ],
        ),
        ("terms", "sk") => (
            "Podmienky používania",
            vec![
                (
                    "Licencie",
                    vec!["Každý dataset uvádza svoju licenciu na svojej stránke v katalógu a tá určuje, ako ho smiete použiť.".into()],
                ),
                (
                    "Iba čítanie",
                    vec!["Konektor dáta nikdy nemení. Nástroje, ktoré by zapisovali, bez prihlásenia odmietne.".into()],
                ),
                (
                    "Bez záruky",
                    vec![format!("{domain} beží ako vývojová služba.")],
                ),
            ],
        ),
        ("terms", _) => (
            "Terms of use",
            vec![
                (
                    "Licences",
                    vec!["Each dataset names its licence on its catalogue page, and that licence governs its reuse.".into()],
                ),
                (
                    "Read only",
                    vec!["The connector never changes data. Tools that would write are refused without a sign-in.".into()],
                ),
                ("As is", vec![format!("{domain} runs as a development service.")]),
            ],
        ),
        ("support", "sk") => (
            "Podpora",
            vec![(
                "",
                vec![
                    "Napíšte nám, keď konektor zlyhá alebo číslo nesedí. Uveďte dataset alebo nástroj, otázku, ktorú ste položili, a čas.".into(),
                    "Chybné číslo v datasete: oznámte ho aj jeho vydavateľovi, uvedenému na stránke datasetu.".into(),
                ],
            )],
        ),
        ("support", _) => (
            "Support",
            vec![(
                "",
                vec![
                    "Write to us when the connector fails or a number looks wrong. Name the dataset or the tool, the question you asked and the time.".into(),
                    "A wrong number in a dataset: tell its publisher too, named on the dataset page.".into(),
                ],
            )],
        ),
        ("accessibility", "sk") => (
            "Vyhlásenie o prístupnosti",
            vec![
                (
                    "",
                    vec!["Chceme, aby tento katalóg mohol používať každý, aj človek, ktorý pracuje iba s klávesnicou, s čítačkou obrazovky alebo so zväčšením.".into()],
                ),
                (
                    "Právny rámec",
                    vec!["Prístupnosť upravuje zákon č. 95/2019 Z. z. o informačných technológiách vo verejnej správe a vyhláška č. 78/2020 Z. z., ktorá za štandard berie WCAG 2.1 na úrovni AA (EN 301 549).".into()],
                ),
                (
                    "Stav súladu",
                    vec!["Katalóg je čiastočne v súlade s WCAG 2.1 na úrovni AA.".into()],
                ),
            ],
        ),
        (_, _) => (
            "Accessibility statement",
            vec![
                (
                    "",
                    vec!["We want everyone to be able to use this catalogue, including people who work only with a keyboard, a screen reader or magnification.".into()],
                ),
                (
                    "Legal basis",
                    vec!["Accessibility is governed by Act No. 95/2019 Coll. and Decree No. 78/2020 Coll., which take WCAG 2.1 level AA (EN 301 549) as the standard.".into()],
                ),
                (
                    "Compliance status",
                    vec!["This catalogue is partially compliant with WCAG 2.1 level AA.".into()],
                ),
            ],
        ),
    };

    let other_pages = STATIC_PAGE_NAMES
        .iter()
        .filter(|&&p| p != name)
        .map(|&p| {
            let t = match (p, code.as_str()) {
                ("connector", "sk") => "MCP konektor",
                ("connector", _) => "MCP connector",
                ("privacy", "sk") => "Ochrana súkromia",
                ("privacy", _) => "Privacy",
                ("terms", "sk") => "Podmienky používania",
                ("terms", _) => "Terms of use",
                ("support", "sk") => "Podpora",
                ("support", _) => "Support",
                ("accessibility", "sk") => "Vyhlásenie o prístupnosti",
                (_, _) => "Accessibility statement",
            };
            (p.to_owned(), t.to_owned())
        })
        .collect();

    Some(StaticPageView {
        name: name.to_owned(),
        title: title.to_owned(),
        sections: raw_sections
            .into_iter()
            .map(|(h, p)| (h.to_owned(), p))
            .collect(),
        email: if branding.contact_email.contains('@') {
            branding.contact_email.clone()
        } else {
            String::new()
        },
        other_pages,
    })
}

/// Step-by-step assistant guide (`GUIDES` / `jc_assistant_guide` in `plugin.py` lines 2280-2525, D6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantGuideView {
    pub id: String,
    pub name: String,
    pub title: String,
    pub help: String,
    pub who: Vec<String>,
    pub steps: Vec<String>,
    pub paste_index: usize,
    pub prompt: String,
    pub example: String,
    pub good: String,
    pub trouble: Vec<(String, String)>,
    pub checked: String,
}

pub const CONNECTOR_NAME: &str = "Joinedcontext Open Data";

pub fn jc_try_prompt(branding: &BrandingBlock, lang: &str) -> String {
    let code = normalize_lang_code(lang);
    let question = branding
        .samples
        .get(&code)
        .and_then(|s| s.first())
        .cloned()
        .unwrap_or_else(|| jc_t("guide_example", lang));
    format!(
        "{}{question}",
        jc_t("guide_use", lang).replace("{connector}", CONNECTOR_NAME)
    )
}

pub fn jc_assistant_guide(id: &str, branding: &BrandingBlock, lang: &str) -> Option<AssistantGuideView> {
    if !matches!(id, "chatgpt" | "claude" | "grok") {
        return None;
    }
    let code = normalize_lang_code(lang);
    let (name, checked_date, paste_index, help, who, steps, trouble): (
        &str,
        &str,
        usize,
        &str,
        Vec<&str>,
        Vec<&str>,
        Vec<(&str, &str)>,
    ) = match (id, code.as_str()) {
        ("claude", "sk") => (
            "Claude",
            "2026-10-18",
            3,
            "https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp",
            vec![
                "Všetky plány Claude, aj bezplatný: bezplatný účet smie pridať jeden vlastný konektor.",
                "V účte Team alebo Enterprise ho raz pridá vlastník organizácie; vy sa k nemu potom iba pripojíte.",
            ],
            vec![
                "Otvorte claude.ai/customize/connectors a prihláste sa.",
                "Kliknite na + a vyberte pridanie vlastného konektora.",
                "Ako názov napíšte Joinedcontext Open Data.",
                "Vložte túto adresu a pokračujte:",
                "Nastavenia, ktoré Claude našiel, nechajte a pokračujte.",
                "Pri prihlásení zvoľte možnosť bez prihlásenia a kliknite na pridať.",
                "V novom rozhovore kliknite vľavo dole na +, otvorte konektory a zapnite Joinedcontext Open Data.",
            ],
            vec![
                ("Claude hlási, že ďalší konektor pridať nemôžete.", "Bezplatný účet má jeden vlastný konektor: odstráňte ten, ktorý už nepoužívate, a pridajte tento."),
                ("Tlačidlo na pridanie tam nie je.", "V účte Team alebo Enterprise požiadajte vlastníka organizácie, aby ho pridal."),
                ("Claude žiada prihlásenie.", "Prihlásenie netreba. Konektor odstráňte a pridajte znova s možnosťou bez prihlásenia: Claude ju neskôr zmeniť nedovolí."),
            ],
        ),
        ("claude", _) => (
            "Claude",
            "2026-10-18",
            3,
            "https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp",
            vec![
                "Every Claude plan, the free one too: a free account may add one connector of its own.",
                "In a Team or Enterprise account, the organization's owner adds it once; you then only connect to it.",
            ],
            vec![
                "Open claude.ai/customize/connectors and sign in.",
                "Click + Add and choose to add your own connector.",
                "Type Joinedcontext Open Data as the name.",
                "Paste this address and continue:",
                "Keep the settings Claude found and continue.",
                "Under sign-in, choose no sign-in, then click Add.",
                "In a new chat, click + at the bottom left, open the connectors and switch Joinedcontext Open Data on.",
            ],
            vec![
                ("Claude says you cannot add another connector.", "A free account keeps one connector of its own: remove the one you no longer use and add this one."),
                ("There is no button to add one.", "In a Team or Enterprise account, ask the organization's owner to add it."),
                ("Claude asks you to sign in.", "No sign-in is needed. Remove the connector and add it again, choosing no sign-in: Claude does not let you change it afterwards."),
            ],
        ),
        ("chatgpt", "sk") => (
            "ChatGPT",
            "2026-10-18",
            3,
            "https://help.openai.com/en/articles/12584461",
            vec![
                "Na chatgpt.com v prehliadači v počítači, s plánom ChatGPT Business, Enterprise alebo Edu.",
                "Iné plány OpenAI neuvádza, Free a Plus to teda zatiaľ nemusia ponúkať.",
            ],
            vec![
                "Na počítači otvorte chatgpt.com, prihláste sa a prejdite na Plugins.",
                "Kliknite na + a pridajte vlastný MCP server.",
                "Ako názov napíšte Joinedcontext Open Data.",
                "Do poľa pre pripojenie vložte túto adresu:",
                "Pri prihlasovaní zvoľte možnosť bez overenia.",
                "Prečítajte si upozornenie, označte, že mu rozumiete, a kliknite na vytvoriť.",
                "V novom rozhovore napíšte @ a vyberte Joinedcontext Open Data.",
            ],
            vec![
                ("Možnosť pridať vlastný MCP server tam nie je.", "Váš plán ju nemá: pozrite, kto to môže použiť."),
                ("ChatGPT sa na adresu nedostane.", "Skopírujte adresu znova tlačidlom a vložte ju celú."),
                ("Odpoveď katalóg nepoužije.", "Otvorte nový rozhovor, napíšte @, vyberte Joinedcontext Open Data a opýtajte sa znova."),
            ],
        ),
        ("chatgpt", _) => (
            "ChatGPT",
            "2026-10-18",
            3,
            "https://help.openai.com/en/articles/12584461",
            vec![
                "On chatgpt.com in a computer's browser, with ChatGPT Business, Enterprise or Edu.",
                "OpenAI names no other plan, so Free and Plus may not offer it yet.",
            ],
            vec![
                "On your computer, open chatgpt.com, sign in and go to Plugins.",
                "Click + and choose Add custom MCP server.",
                "Type Joinedcontext Open Data as the name.",
                "Paste this address into the connection field:",
                "Under sign-in, choose no authentication.",
                "Read the warning, tick that you understand, and click Create.",
                "In a new chat type @ and choose Joinedcontext Open Data.",
            ],
            vec![
                ("There is no option to add a custom MCP server.", "Your plan does not offer it: see who can use it above."),
                ("ChatGPT cannot reach the address.", "Copy the address again with the button and paste it whole."),
                ("The answer does not use the catalogue.", "Start a new chat, type @, pick Joinedcontext Open Data and ask again."),
            ],
        ),
        (_, "sk") => (
            "Grok",
            "2026-10-18",
            2,
            "https://docs.x.ai/grok/connectors",
            vec![
                "Každý používateľ Groku, na grok.com; v telefóne je tá istá stránka v Nastaveniach, v časti Konektory.",
            ],
            vec![
                "Otvorte grok.com/connectors a prihláste sa.",
                "Kliknite na nový konektor a vyberte vlastný.",
                "Vložte túto adresu a potvrďte:",
                "Otvorte nový rozhovor a položte otázku.",
            ],
            vec![
                ("Grok adresu odmietne.", "Skopírujte adresu znova tlačidlom a vložte ju celú."),
                ("Odpoveď katalóg nepoužije.", "Otvorte nový rozhovor a napíšte: použi Joinedcontext Open Data."),
            ],
        ),
        (_, _) => (
            "Grok",
            "2026-10-18",
            2,
            "https://docs.x.ai/grok/connectors",
            vec![
                "Every Grok user, on grok.com; on a phone the same page is under Settings, Connectors.",
            ],
            vec![
                "Open grok.com/connectors and sign in.",
                "Click New Connector and choose Custom.",
                "Paste this address and confirm:",
                "Start a new chat and ask your question.",
            ],
            vec![
                ("Grok refuses the address.", "Copy the address again with the button and paste it whole."),
                ("The answer does not use the catalogue.", "Start a new chat and write: use Joinedcontext Open Data."),
            ],
        ),
    };

    let prompt = jc_try_prompt(branding, lang);
    Some(AssistantGuideView {
        id: id.to_owned(),
        name: name.to_owned(),
        title: jc_t("guide_title", lang).replace("{name}", name),
        help: help.to_owned(),
        who: who.into_iter().map(str::to_owned).collect(),
        steps: steps.into_iter().map(str::to_owned).collect(),
        paste_index,
        example: prompt.clone(),
        prompt,
        good: jc_t("guide_good", lang).replace("{name}", CONNECTOR_NAME),
        trouble: trouble
            .into_iter()
            .map(|(p, f)| (p.to_owned(), f.to_owned()))
            .collect(),
        checked: jc_t("guide_checked", lang)
            .replace("{date}", checked_date)
            .replace("{maker}", name),
    })
}

/// Generates `sitemap.xml` for public non-hidden datasets and active publishers (`jc_sitemap_xml` in `plugin.py`, D6).
pub fn jc_sitemap_xml(
    cat: &Catalogue,
    datasets: &[Dataset],
    publishers: &[Publisher],
) -> String {
    let site = cat.base_url();
    let mut lines = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_owned(),
        "<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned(),
    ];
    let mut pub_ds: Vec<&Dataset> = datasets
        .iter()
        .filter(|d| {
            d.state == RecordState::Active
                && d.visibility == Visibility::Public
                && !cat.hidden_datasets.contains_key(&d.name)
        })
        .collect();
    pub_ds.sort_by(|a, b| a.name.cmp(&b.name));

    for ds in pub_ds.into_iter().take(50_000) {
        let loc = html_escape(&format!("{site}/dataset/{}", ds.name));
        let lastmod = ds.modified.format("%Y-%m-%d").to_string();
        lines.push(format!("<url><loc>{loc}</loc><lastmod>{lastmod}</lastmod></url>"));
    }
    for p in publishers.iter().filter(|p| p.state == RecordState::Active) {
        let loc = html_escape(&format!("{site}/organization/{}", p.name));
        lines.push(format!("<url><loc>{loc}</loc></url>"));
    }
    lines.push("</urlset>\n".to_owned());
    lines.join("\n")
}

/// Generates `robots.txt` (`jc_robots_txt` in `plugin.py` line 1689, B4, D6):
/// when `branding.noindex` is `true`, returns `Disallow: /`.
pub fn jc_robots_txt(cat: &Catalogue, branding: &BrandingBlock) -> String {
    if branding.noindex {
        return "User-agent: *\nDisallow: /\n".to_owned();
    }
    let site = cat.base_url();
    format!(
        "User-agent: *\nDisallow: /dataset/rate/\nDisallow: /revision/\nDisallow: /dataset/*/history\nDisallow: /api/\nCrawl-Delay: 10\n\nSitemap: {site}/sitemap.xml\n"
    )
}

/// Resolves publisher display card info (`jc_org` in `plugin.py`).
pub fn publisher_card_info(
    p: &Publisher,
    lang: &str,
    default_lang: &str,
) -> (String, String, Option<String>) {
    let title = localized_text(&p.title, lang, default_lang)
        .unwrap_or(&p.name)
        .to_owned();
    let fb = PublisherBranding {
        initials: p.badge_initials.clone().or_else(|| {
            let init: String = p
                .name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(3)
                .map(|c| c.to_ascii_uppercase())
                .collect();
            (!init.is_empty()).then_some(init)
        }),
        colour: p
            .badge_colour
            .clone()
            .or_else(|| Some("#00579c".into())),
        logo_url: p.logo_url.clone(),
        description: p.description.clone(),
        homepage: p.homepage.clone(),
    };
    let (img, desc) =
        resolve_publisher_branding(p.logo_url.as_deref(), &p.description, Some(&fb), lang, default_lang);
    let image = img.unwrap_or_else(|| "/base/images/placeholder-organization.png".into());
    let _ = jc_rows;
    (title, image, desc)
}
