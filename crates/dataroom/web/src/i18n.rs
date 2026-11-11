#![forbid(unsafe_code)]
//! Languages, URL prefix / cookie / `Accept-Language` / default resolution, and localized string
//! bundles (`sk`, `cs`, `en`, `hu`, `de`, C1-C4).
//!
//! Ported from `STRINGS`, `FORMATS`, `FREQUENCIES`, `TOPICS`, `MODEL_FILES`, `EXTRA_LABELS`,
//! `GUIDES`, and `PAGES` in `ours/ckan-theme/plugin.py`.

use std::collections::BTreeMap;

/// The languages shipped with complete UI bundles in dataroom (C1).
pub const SHIPPED_LANGUAGES: [&str; 5] = ["sk", "cs", "en", "hu", "de"];

/// Display names of languages in the footer language switcher (matching `live/*.html`).
pub fn language_display_name(code: &str) -> &'static str {
    match code {
        "sk" => "slovenčina",
        "cs" => "čeština",
        "en" => "English",
        "hu" => "magyar",
        "de" => "Deutsch",
        "fi" => "suomi",
        _ => "English",
    }
}

/// Filters a catalogue's `offered` language list to the languages that dataroom actually ships,
/// preserving order and deduplicating (`_available` in `plugin.py` lines 101-120, C1).
/// Never fails if an unknown language code is listed in `offered`.
pub fn available_languages(offered: &[String]) -> Vec<String> {
    let mut seen = Vec::new();
    for code in offered {
        let norm = normalize_lang_code(code);
        if SHIPPED_LANGUAGES.contains(&norm.as_str()) && !seen.contains(&norm) {
            seen.push(norm);
        }
    }
    if seen.is_empty() {
        seen.push("en".into());
    }
    seen
}

/// Normalizes a locale tag like `cs_CZ` or `sk-SK` to its 2-letter base code (`_language` in `plugin.py`).
pub fn normalize_lang_code(raw: &str) -> String {
    raw.trim()
        .split(['_', '-'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

/// Outcome of resolving a request's language from URL prefix, cookie, `Accept-Language`, and default (C2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageResolution {
    /// Resolved `(lang, stripped_path, had_url_prefix)`.
    Resolved {
        lang: String,
        stripped_path: String,
        had_url_prefix: bool,
    },
    /// The URL started with a 2-letter prefix `/{xx}/...` that is not a known/shipped language -> 404 (C2).
    UnknownUrlPrefix(String),
}

/// Known top-level route first segments that are NOT language prefixes.
const ROUTE_SEGMENTS: &[&str] = &[
    "dataset",
    "organization",
    "publisher",
    "group",
    "about",
    "assistants",
    "connector",
    "privacy",
    "terms",
    "support",
    "accessibility",
    "map",
    "user",
    "util",
    "api",
    "odata",
    "feeds",
    "profiles",
    "catalog",
    "catalog.ttl",
    "catalog.jsonld",
    "catalog.rdf",
    "catalog.xml",
    "catalog.nt",
    "sitemap.xml",
    "robots.txt",
    "mcp",
    "assets",
    "webassets",
];

/// Resolves the active language in strict C2 priority order:
/// 1. URL prefix `/{lang}/...` or `/{lang}` (e.g. `/sk/dataset/air-quality` or `/sk/`).
///    If the first path segment looks like a 2-letter language prefix (`[a-z]{2}`) that is not a
///    route name and is not in `SHIPPED_LANGUAGES` (or `offered`), returns `UnknownUrlPrefix` (404).
/// 2. Cookie (`ckan_lang` / `jc_lang` / `lang`).
/// 3. `Accept-Language` header.
/// 4. Catalogue default language.
pub fn resolve_request_language(
    path: &str,
    cookie_header: Option<&str>,
    accept_language: Option<&str>,
    offered: &[String],
    default_lang: &str,
) -> LanguageResolution {
    let avail = available_languages(offered);
    let def = {
        let d = normalize_lang_code(default_lang);
        if avail.contains(&d) || SHIPPED_LANGUAGES.contains(&d.as_str()) {
            d
        } else {
            avail.first().cloned().unwrap_or_else(|| "en".into())
        }
    };

    let clean_path = if path.is_empty() { "/" } else { path };
    let trimmed = clean_path.trim_start_matches('/');
    let (first_seg, rest_opt) = match trimmed.split_once('/') {
        Some((first, rest)) => (first, Some(format!("/{rest}"))),
        None => (trimmed, None),
    };

    if !first_seg.is_empty() && !ROUTE_SEGMENTS.contains(&first_seg) && !first_seg.contains('.') {
        let lower = first_seg.to_ascii_lowercase();
        if SHIPPED_LANGUAGES.contains(&lower.as_str()) {
            return LanguageResolution::Resolved {
                lang: lower,
                stripped_path: rest_opt.unwrap_or_else(|| "/".into()),
                had_url_prefix: true,
            };
        }
        // Any 2-letter alphabetic segment that is not a route or shipped language is an unknown
        // language prefix and must 404 (C2).
        if first_seg.len() == 2 && first_seg.bytes().all(|b| b.is_ascii_alphabetic()) {
            return LanguageResolution::UnknownUrlPrefix(first_seg.to_owned());
        }
    }

    // 2. Cookie
    if let Some(cookies) = cookie_header {
        for pair in cookies.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                let k = k.trim();
                if matches!(k, "ckan_lang" | "jc_lang" | "lang") {
                    let c = normalize_lang_code(v);
                    if avail.contains(&c) || SHIPPED_LANGUAGES.contains(&c.as_str()) {
                        return LanguageResolution::Resolved {
                            lang: c,
                            stripped_path: clean_path.to_owned(),
                            had_url_prefix: false,
                        };
                    }
                }
            }
        }
    }

    // 3. Accept-Language
    if let Some(al) = accept_language {
        let mut candidates: Vec<(String, f32)> = Vec::new();
        for item in al.split(',') {
            let mut parts = item.trim().split(';');
            let Some(tag) = parts.next() else {
                continue;
            };
            let code = normalize_lang_code(tag);
            let mut q = 1.0_f32;
            for param in parts {
                if let Some(q_str) = param.trim().strip_prefix("q=") {
                    if let Ok(parsed_q) = q_str.parse::<f32>() {
                        q = parsed_q;
                    }
                }
            }
            if q > 0.0 {
                candidates.push((code, q));
            }
        }
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (code, _) in candidates {
            if avail.contains(&code) || SHIPPED_LANGUAGES.contains(&code.as_str()) {
                return LanguageResolution::Resolved {
                    lang: code,
                    stripped_path: clean_path.to_owned(),
                    had_url_prefix: false,
                };
            }
        }
    }

    // 4. Default
    LanguageResolution::Resolved {
        lang: def,
        stripped_path: clean_path.to_owned(),
        had_url_prefix: false,
    }
}

/// Builds the localized URL for `lang` preserving the current page path and query.
pub fn url_in_lang(
    mount_prefix: &str,
    lang: &str,
    stripped_path: &str,
    query: Option<&str>,
) -> String {
    let pfx = mount_prefix.trim_end_matches('/');
    let p = if stripped_path.starts_with('/') {
        stripped_path.to_owned()
    } else {
        format!("/{stripped_path}")
    };
    let base = format!("{pfx}/{lang}{p}");
    match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{base}?{q}"),
        None => base,
    }
}

/// Complete dictionary of UI strings from `ours/ckan-theme/plugin.py` `STRINGS` + CKAN template
/// words in `sk`, `cs`, `en`, `hu`, and `de` (C3).
pub fn string_table() -> BTreeMap<&'static str, BTreeMap<&'static str, &'static str>> {
    let mut m: BTreeMap<&'static str, BTreeMap<&'static str, &'static str>> = BTreeMap::new();
    let mut add = |key: &'static str,
                   en: &'static str,
                   sk: &'static str,
                   cs: &'static str,
                   de: &'static str,
                   hu: &'static str| {
        m.insert(
            key,
            BTreeMap::from([("en", en), ("sk", sk), ("cs", cs), ("de", de), ("hu", hu)]),
        );
    };

    // Every entry from `STRINGS` in `ours/ckan-theme/plugin.py` lines 234-420:
    add(
        "about",
        "About this dataset",
        "O tomto datasete",
        "O této datové sadě",
        "Über diesen Datensatz",
        "Az adatkészletről",
    );
    add(
        "publisher",
        "Publisher",
        "Vydavateľ",
        "Vydavatel",
        "Herausgeber",
        "Kiadó",
    );
    add(
        "licence", "Licence", "Licencia", "Licence", "Lizenz", "Licenc",
    );
    add(
        "attribution",
        "Attribution",
        "Uvedenie zdroja",
        "Uvedení zdroje",
        "Quellenangabe",
        "Forrásmegjelölés",
    );
    add(
        "source",
        "Original data",
        "Pôvodné dáta",
        "Původní data",
        "Ursprüngliche Daten",
        "Eredeti adatok",
    );
    add(
        "sources_all",
        "Show all ({n})",
        "Zobraziť všetky ({n})",
        "Zobrazit všechny ({n})",
        "Alle anzeigen ({n})",
        "Mind megjelenítése ({n})",
    );
    add("topic", "Topic", "Téma", "Téma", "Thema", "Téma");
    add(
        "keywords",
        "Keywords",
        "Kľúčové slová",
        "Klíčová slova",
        "Schlagwörter",
        "Kulcsszavak",
    );
    add("format", "Format", "Formát", "Formát", "Format", "Formátum");
    add(
        "by_topic",
        "Browse by topic",
        "Podľa témy",
        "Podle tématu",
        "Nach Thema",
        "Témák szerint",
    );
    add(
        "frequency",
        "Updated",
        "Aktualizácia",
        "Aktualizace",
        "Aktualisierung",
        "Frissítés",
    );
    add(
        "last_update",
        "Last update",
        "Posledná aktualizácia",
        "Poslední aktualizace",
        "Letzte Aktualisierung",
        "Utolsó frissítés",
    );
    add(
        "next_update",
        "Next update expected",
        "Ďalšia aktualizácia",
        "Další aktualizace",
        "Nächste Aktualisierung",
        "Következő várható frissítés",
    );
    add(
        "coverage",
        "Area covered",
        "Pokryté územie",
        "Pokryté území",
        "Abgedecktes Gebiet",
        "Lefedett terület",
    );
    add(
        "period",
        "Time covered",
        "Obdobie",
        "Období",
        "Zeitraum",
        "Lefedett időszak",
    );
    add(
        "contact",
        "Contact",
        "Kontakt",
        "Kontakt",
        "Kontakt",
        "Kapcsolat",
    );
    add(
        "identifier",
        "Identifier",
        "Identifikátor",
        "Identifikátor",
        "Kennung",
        "Azonosító",
    );
    add("language", "Language", "Jazyk", "Jazyk", "Sprache", "Nyelv");
    add(
        "conforms_to",
        "Follows the standard",
        "Zodpovedá štandardu",
        "Odpovídá standardu",
        "Folgt dem Standard",
        "Megfelel a szabványnak",
    );
    add("status", "Status", "Stav", "Stav", "Status", "Állapot");
    add(
        "live",
        "Live data (API)",
        "Živé dáta (API)",
        "Živá data (API)",
        "Live-Daten (API)",
        "Élő adatok (API)",
    );
    add(
        "live_lead",
        "The current state of this data, answered by the platform on every request (NGSI-LD).",
        "Aktuálny stav týchto dát, ktorý platforma vracia pri každej požiadavke (NGSI-LD).",
        "Aktuální stav těchto dat, který platforma vrací při každém požadavku (NGSI-LD).",
        "Der aktuelle Stand dieser Daten, von der Plattform bei jeder Anfrage beantwortet (NGSI-LD).",
        "Az adatok aktuális állapota, amelyet a platform minden kérésre megad (NGSI-LD).",
    );
    add(
        "example",
        "Start here: the kinds of entities it holds",
        "Začnite tu: aké druhy entít obsahuje",
        "Začněte zde: jaké druhy entit obsahuje",
        "Hier beginnen: welche Arten von Entitäten es enthält",
        "Kezdje itt: milyen entitásokat tartalmaz",
    );
    add(
        "copy",
        "Copy",
        "Kopírovať",
        "Kopírovat",
        "Kopieren",
        "Másolás",
    );
    add(
        "copied",
        "Copied",
        "Skopírované",
        "Zkopírováno",
        "Kopiert",
        "Másolva",
    );
    add(
        "copy_select",
        "Selected: press Ctrl+C",
        "Označené: stlačte Ctrl+C",
        "Označeno: stiskněte Ctrl+C",
        "Markiert: Strg+C drücken",
        "Kijelölve: nyomja meg a Ctrl+C billentyűket",
    );
    add(
        "visualizations",
        "Charts",
        "Grafy",
        "Grafy",
        "Diagramme",
        "Grafikonok",
    );
    add(
        "viz_embed",
        "Embed on a website",
        "Vložiť na web",
        "Vložit na web",
        "Auf einer Website einbetten",
        "Beágyazás weboldalba",
    );
    add(
        "viz_embed_lead",
        "Paste this code into your page; the chart keeps reading the current data.",
        "Vložte tento kód do svojej stránky; graf bude stále čítať aktuálne dáta.",
        "Vložte tento kód do své stránky; graf bude stále číst aktuální data.",
        "Fügen Sie diesen Code in Ihre Seite ein; das Diagramm liest weiter die aktuellen Daten.",
        "Illessze be ezt a kódot az oldalába; a grafikon mindig az aktuális adatokat olvassa.",
    );
    add(
        "viz_table",
        "The data as a table",
        "Údaje ako tabuľka",
        "Údaje jako tabulka",
        "Die Daten als Tabelle",
        "Az adatok táblázatként",
    );
    add(
        "resources",
        "Data and downloads",
        "Dáta na stiahnutie",
        "Data ke stažení",
        "Daten und Downloads",
        "Adatok és letöltések",
    );
    add(
        "no_resources",
        "This dataset has no downloads yet.",
        "Tento dataset zatiaľ nemá nič na stiahnutie.",
        "Tato datová sada zatím nemá nic ke stažení.",
        "Dieser Datensatz hat noch keine Downloads.",
        "Ehhez az adatkészlethez még nincs letölthető fájl.",
    );
    add(
        "empty",
        "Nothing matches. Try fewer or other words, or remove a filter on the left.",
        "Nič nezodpovedá. Skúste menej alebo iné slová, alebo zrušte filter vľavo.",
        "Nic neodpovídá. Zkuste méně nebo jiná slova, nebo zrušte filtr vlevo.",
        "Nichts gefunden. Versuchen Sie weniger oder andere Wörter, oder entfernen Sie links einen Filter.",
        "Nincs találat. Próbáljon kevesebb vagy más szót, vagy törölje a bal oldali szűrőt.",
    );
    add("portal", "Portal", "Portál", "Portál", "Portal", "Portál");
    add(
        "about_site",
        "About this catalogue",
        "O tomto katalógu",
        "O tomto katalogu",
        "Über diesen Katalog",
        "A katalógusról",
    );
    add(
        "catalogue_api",
        "Catalogue API",
        "API katalógu",
        "API katalogu",
        "Katalog-API",
        "Katalógus API",
    );
    add(
        "assistant",
        "Ask the assistant",
        "Opýtajte sa asistenta",
        "Zeptejte se asistenta",
        "Fragen Sie den Assistenten",
        "Kérdezze az asszisztenst",
    );
    add(
        "preview",
        "The data",
        "Dáta",
        "Data",
        "Die Daten",
        "Az adatok",
    );
    add(
        "explorer",
        "Live data",
        "Živé dáta",
        "Živá data",
        "Live-Daten",
        "Élő adatok",
    );
    add(
        "explorer_lead",
        "Search, filter and map the entities straight from the published Endpoint; nothing is copied, so you always see the current data.",
        "Hľadajte, filtrujte a zobrazte entity na mape priamo zo zverejneného Endpointu; nič sa nekopíruje, takže vždy vidíte aktuálne dáta.",
        "Hledejte, filtrujte a zobrazte entity na mapě přímo ze zveřejněného Endpointu; nic se nekopíruje, takže vždy vidíte aktuální data.",
        "Entitäten direkt am veröffentlichten Endpoint suchen, filtern und auf der Karte zeigen; nichts wird kopiert, Sie sehen immer den aktuellen Stand.",
        "Keressen, szűrjön és jelenítse meg térképen az entitásokat közvetlenül a közzétett Endpointból; semmi sem másolódik, így mindig az aktuális adatokat látja.",
    );
    add(
        "preview_lead",
        "Browse, search and sort the rows here, or download the whole table.",
        "Prezerajte, vyhľadávajte a triedte riadky priamo tu, alebo si stiahnite celú tabuľku.",
        "Procházejte, vyhledávejte a řaďte řádky přímo zde, nebo si stáhněte celou tabulku.",
        "Zeilen hier durchsuchen, filtern und sortieren oder die ganze Tabelle herunterladen.",
        "Böngéssze, keresse és rendezze itt a sorokat, vagy töltse le a teljes táblázatot.",
    );
    add(
        "table_title",
        "Table of the data",
        "Tabuľka dát",
        "Tabulka dat",
        "Tabelle der Daten",
        "Az adatok táblázata",
    );
    add("map", "Map", "Mapa", "Mapa", "Karte", "Térkép");
    add(
        "map_title",
        "Map of the data",
        "Mapa dát",
        "Mapa dat",
        "Karte der Daten",
        "Az adatok térképe",
    );
    add(
        "map_as_table",
        "Show as a table",
        "Zobraziť ako tabuľku",
        "Zobrazit jako tabulku",
        "Als Tabelle anzeigen",
        "Megjelenítés táblázatként",
    );
    add(
        "map_in_catalogue",
        "Show in the catalogue",
        "Zobraziť v katalógu",
        "Zobrazit v katalogu",
        "Im Katalog anzeigen",
        "Megjelenítés a katalógusban",
    );
    add(
        "map_embed",
        "Embed on your site",
        "Vložiť na web",
        "Vložit na web",
        "Auf Ihrer Website einbetten",
        "Beágyazás a saját oldalába",
    );
    add(
        "map_embed_lead",
        "Paste this into your page; the map stays live and shows only public data.",
        "Vložte to do svojej stránky; mapa zostáva živá a ukazuje len verejné údaje.",
        "Vložte to do své stránky; mapa zůstává živá a ukazuje jen veřejná data.",
        "Fügen Sie dies in Ihre Seite ein; die Karte bleibt aktuell und zeigt nur öffentliche Daten.",
        "Illessze be ezt az oldalába; a térkép élő marad, és csak nyilvános adatokat mutat.",
    );
    add(
        "map_cap",
        "Showing the first {n}; zoom in to see the rest.",
        "Zobrazených prvých {n}, priblížte mapu.",
        "Zobrazeno prvních {n}, přibližte mapu.",
        "Die ersten {n} werden gezeigt; zoomen Sie hinein.",
        "Az első {n} látható; nagyítson rá a többi megtekintéséhez.",
    );
    add(
        "map_failed",
        "The map could not load its data; the table above still works.",
        "Mapa nenačítala údaje; tabuľka vyššie funguje.",
        "Mapa nenačetla data; tabulka výše funguje.",
        "Die Karte konnte ihre Daten nicht laden; die Tabelle oben funktioniert.",
        "A térkép nem tudta betölteni az adatait; a fenti táblázat továbbra is működik.",
    );
    add(
        "map_empty",
        "This dataset has no places to show on a map.",
        "Tento dataset nemá miesta, ktoré by sa dali ukázať na mape.",
        "Tato datová sada nemá místa, která by šla ukázat na mapě.",
        "Dieser Datensatz enthält keine Orte, die eine Karte zeigen könnte.",
        "Ebben az adatkészletben nincsenek térképen megjeleníthető helyek.",
    );
    add(
        "map_close",
        "Close",
        "Zavrieť",
        "Zavřít",
        "Schließen",
        "Bezárás",
    );
    add("map_more", "More", "Viac", "Více", "Mehr", "Továbbiak");
    add(
        "map_loading",
        "Loading the map's data…",
        "Načítavam údaje mapy…",
        "Načítám data mapy…",
        "Die Kartendaten werden geladen…",
        "A térkép adatai betöltődnek…",
    );
    add(
        "map_panel",
        "Selected place",
        "Vybrané miesto",
        "Vybrané místo",
        "Ausgewählter Ort",
        "Kiválasztott hely",
    );
    add("map_length", "Length", "Dĺžka", "Délka", "Länge", "Hossz");
    add("map_area", "Area", "Plocha", "Plocha", "Fläche", "Terület");
    add(
        "map_keys",
        "Arrow keys move the map, + and - zoom, Enter opens the place in the middle.",
        "Šípky posúvajú mapu, + a - menia priblíženie, Enter otvorí miesto v strede.",
        "Šipky posouvají mapu, + a - mění přiblížení, Enter otevře místo uprostřed.",
        "Pfeiltasten verschieben die Karte, + und - zoomen, Enter öffnet den Ort in der Mitte.",
        "A nyílbillentyűk mozgatják a térképet, a + és a - nagyít és kicsinyít, az Enter megnyitja a középen lévő helyet.",
    );
    add(
        "tables",
        "tables",
        "tabuľky",
        "tabulky",
        "Tabellen",
        "táblázat",
    );
    add(
        "table_what",
        "The table above, as one file",
        "Tabuľka vyššie ako jeden súbor",
        "Tabulka výše jako jeden soubor",
        "Die Tabelle oben als eine Datei",
        "A fenti táblázat egy fájlban",
    );
    add(
        "open_table",
        "Open the table on its own page",
        "Otvoriť tabuľku na samostatnej stránke",
        "Otevřít tabulku na samostatné stránce",
        "Tabelle auf eigener Seite öffnen",
        "A táblázat megnyitása külön oldalon",
    );
    add(
        "no_view",
        "The table has no view yet; download it below.",
        "Tabuľka zatiaľ nemá zobrazenie; stiahnite si ju nižšie.",
        "Tabulka zatím nemá zobrazení; stáhněte si ji níže.",
        "Die Tabelle hat noch keine Ansicht; laden Sie sie unten herunter.",
        "A táblázatnak még nincs nézete; töltse le lent.",
    );
    add(
        "download",
        "Download",
        "Stiahnuť",
        "Stáhnout",
        "Herunterladen",
        "Letöltés",
    );
    add(
        "part_of",
        "Part of the dataset",
        "Súčasť datasetu",
        "Součást datové sady",
        "Teil des Datensatzes",
        "Az adatkészlet része",
    );
    add("address", "Address", "Adresa", "Adresa", "Adresse", "Cím");
    add(
        "file_lead",
        "This is a file to download; it opens in the program that reads its format.",
        "Toto je súbor na stiahnutie; otvorí sa v programe, ktorý číta jeho formát.",
        "Toto je soubor ke stažení; otevře se v programu, který čte jeho formát.",
        "Dies ist eine Datei zum Herunterladen; sie öffnet sich im Programm, das ihr Format liest.",
        "Ez egy letölthető fájl; a formátumát olvasó programban nyílik meg.",
    );
    add(
        "api_lead",
        "This is an address a program calls; open it to see what it answers.",
        "Toto je adresa, ktorú volá program; otvorte ju a uvidíte, čo odpovedá.",
        "Toto je adresa, kterou volá program; otevřete ji a uvidíte, co odpovídá.",
        "Dies ist eine Adresse, die ein Programm aufruft; öffnen Sie sie, um die Antwort zu sehen.",
        "Ez egy cím, amelyet egy program hív meg; nyissa meg, és meglátja, mit válaszol.",
    );
    add("open", "Open", "Otvoriť", "Otevřít", "Öffnen", "Megnyitás");
    add(
        "details",
        "Details",
        "Podrobnosti",
        "Podrobnosti",
        "Details",
        "Részletek",
    );
    add(
        "section_data",
        "Downloads",
        "Na stiahnutie",
        "Ke stažení",
        "Downloads",
        "Letöltések",
    );
    add(
        "section_data_lead",
        "The data as files, updated with the dataset.",
        "Dáta ako súbory, aktualizované spolu s datasetom.",
        "Data jako soubory, aktualizovaná spolu s datovou sadou.",
        "Die Daten als Dateien, mit dem Datensatz aktualisiert.",
        "Az adatok fájlokként, az adatkészlettel együtt frissítve.",
    );
    add(
        "section_api",
        "API and live access",
        "API a živý prístup",
        "API a živý přístup",
        "API und Live-Zugriff",
        "API és élő hozzáférés",
    );
    add(
        "section_api_lead",
        "For applications and AI assistants that query the data live.",
        "Pre aplikácie a AI asistentov, ktorí sa na dáta pýtajú naživo.",
        "Pro aplikace a AI asistenty, kteří se na data ptají živě.",
        "Für Anwendungen und KI-Assistenten, die die Daten live abfragen.",
        "Alkalmazásoknak és AI-asszisztenseknek, amelyek élőben kérdezik le az adatokat.",
    );
    add(
        "section_schema",
        "Data model",
        "Dátový model",
        "Datový model",
        "Datenmodell",
        "Adatmodell",
    );
    add(
        "section_schema_lead",
        "What each field means, in the formats validators and developers read.",
        "Čo znamená každé pole, vo formátoch pre validátory a vývojárov.",
        "Co znamená každé pole, ve formátech pro validátory a vývojáře.",
        "Was jedes Feld bedeutet, in den Formaten für Validatoren und Entwickler.",
        "Mit jelent az egyes mezők, a validátorok és fejlesztők által olvasott formátumokban.",
    );
    add(
        "section_source",
        "At the publisher",
        "U vydavateľa",
        "U vydavatele",
        "Beim Herausgeber",
        "A kiadónál",
    );
    add(
        "section_source_lead",
        "Where the publisher keeps the original.",
        "Kde má vydavateľ originál.",
        "Kde má vydavatel originál.",
        "Wo der Herausgeber das Original hat.",
        "Ahol a kiadó az eredetit tárolja.",
    );
    add(
        "source_lead",
        "This is the publisher's own page for the data; it opens on their site.",
        "Toto je stránka vydavateľa s týmito dátami; otvorí sa na jeho webe.",
        "Toto je stránka vydavatele s těmito daty; otevře se na jeho webu.",
        "Dies ist die Seite des Herausgebers zu diesen Daten; sie öffnet sich auf seiner Website.",
        "Ez a kiadó saját oldala az adatokhoz; az ő webhelyén nyílik meg.",
    );
    add(
        "at_publisher",
        "On the publisher's site",
        "Na stránke vydavateľa",
        "Na stránce vydavatele",
        "Auf der Seite des Herausgebers",
        "A kiadó oldalán",
    );
    add(
        "usage",
        "How to use it",
        "Ako to použiť",
        "Jak to použít",
        "So verwenden Sie es",
        "Hogyan használja",
    );
    add(
        "guide",
        "The WebSocket guide: messages, limits and examples",
        "Príručka k WebSocketu: správy, limity a príklady",
        "Průvodce WebSocketem: zprávy, limity a příklady",
        "Die WebSocket-Anleitung: Nachrichten, Grenzen und Beispiele",
        "A WebSocket-útmutató: üzenetek, korlátok és példák",
    );
    add("developer_page", "Developer page: every surface with code samples, try-it, licence and rate limit", "Stránka pre vývojárov: každé rozhranie s ukážkami kódu, vyskúšaním, licenciou a limitom požiadaviek", "Stránka pro vývojáře: každé rozhraní s ukázkami kódu, vyzkoušením, licencí a limitem požadavků", "Entwicklerseite: jede Schnittstelle mit Codebeispielen, Ausprobieren, Lizenz und Anfragelimit", "Fejlesztői oldal: minden interfész kódpéldákkal, kipróbálással, licenccel és kéréskorláttal");
    add(
        "snippet",
        "Command to copy",
        "Príkaz na skopírovanie",
        "Příkaz ke zkopírování",
        "Befehl zum Kopieren",
        "Másolható parancs",
    );
    add(
        "mcp",
        "Connect an AI assistant",
        "Pripojte AI asistenta",
        "Připojte AI asistenta",
        "KI-Assistenten verbinden",
        "AI-asszisztens csatlakoztatása",
    );
    add(
        "mcp_lead",
        "One address for the whole catalogue (MCP). Add it once and your assistant finds every dataset, its attributes, data model and access rules by itself. Open data needs no login; signing in adds the data you may read.",
        "Jedna adresa pre celý katalóg (MCP). Pridajte ju raz a asistent sám nájde každý dataset, jeho atribúty, dátový model a pravidlá prístupu. Otvorené dáta nepotrebujú prihlásenie; po prihlásení pribudnú dáta, ktoré smiete čítať.",
        "Jedna adresa pro celý katalog (MCP). Přidejte ji jednou a asistent sám najde každou datovou sadu, její atributy, datový model a pravidla přístupu. Otevřená data nepotřebují přihlášení; po přihlášení přibudou data, která smíte číst.",
        "Eine Adresse für den ganzen Katalog (MCP). Einmal hinzugefügt, findet Ihr Assistent jeden Datensatz, seine Attribute, sein Datenmodell und seine Zugriffsregeln selbst. Offene Daten brauchen keine Anmeldung; nach der Anmeldung kommen die Daten hinzu, die Sie lesen dürfen.",
        "Egyetlen cím az egész katalógushoz (MCP). Adja hozzá egyszer, és az asszisztense magától megtalál minden adatkészletet, azok attribútumait, adatmodelljét és hozzáférési szabályait. A nyílt adatokhoz nem kell bejelentkezni; bejelentkezés után azok az adatok is elérhetők, amelyeket Ön olvashat.",
    );
    add(
        "mcp_url",
        "MCP server address",
        "Adresa MCP servera",
        "Adresa MCP serveru",
        "Adresse des MCP-Servers",
        "Az MCP-szerver címe",
    );
    add(
        "mcp_claude_ai",
        "Claude (claude.ai and Claude Desktop)",
        "Claude (claude.ai a Claude Desktop)",
        "Claude (claude.ai a Claude Desktop)",
        "Claude (claude.ai und Claude Desktop)",
        "Claude (claude.ai és Claude Desktop)",
    );
    add("mcp_claude_ai_1", "Open Customize, then Connectors, choose + Add and Add custom connector.", "Otvorte Prispôsobiť, potom Konektory, zvoľte + Pridať a Pridať vlastný konektor.", "Otevřete Přizpůsobit, pak Konektory, zvolte + Přidat a Přidat vlastní konektor.", "Öffnen Sie Anpassen, dann Konnektoren, wählen Sie + Hinzufügen und Benutzerdefinierten Konnektor hinzufügen.", "Nyissa meg a Testreszabás, majd a Csatlakozók menüt, válassza a + Hozzáadás, majd az Egyéni csatlakozó hozzáadása lehetőséget.");
    add(
        "mcp_claude_ai_2",
        "Name it joinedcontext and paste this address:",
        "Pomenujte ho joinedcontext a vložte túto adresu:",
        "Pojmenujte ho joinedcontext a vložte tuto adresu:",
        "Nennen Sie ihn joinedcontext und fügen Sie diese Adresse ein:",
        "Nevezze el joinedcontext néven, és illessze be ezt a címet:",
    );
    add("mcp_claude_ai_3", "Leave the advanced settings empty and choose Add. Open data needs no login.", "Rozšírené nastavenia nechajte prázdne a zvoľte Pridať. Otvorené dáta nepotrebujú prihlásenie.", "Rozšířená nastavení nechte prázdná a zvolte Přidat. Otevřená data nepotřebují přihlášení.", "Lassen Sie die erweiterten Einstellungen leer und wählen Sie Hinzufügen. Offene Daten brauchen keine Anmeldung.", "A speciális beállításokat hagyja üresen, és válassza a Hozzáadás lehetőséget. A nyílt adatokhoz nem kell bejelentkezni.");
    add(
        "mcp_editors",
        "Code editors and other assistants",
        "Editory kódu a iní asistenti",
        "Editory kódu a jiní asistenti",
        "Code-Editoren und andere Assistenten",
        "Kódszerkesztők és más asszisztensek",
    );
    add(
        "mcp_claude",
        "Claude Code",
        "Claude Code",
        "Claude Code",
        "Claude Code",
        "Claude Code",
    );
    add(
        "mcp_json",
        "Any other MCP client (JSON)",
        "Iný MCP klient (JSON)",
        "Jiný MCP klient (JSON)",
        "Jeder andere MCP-Client (JSON)",
        "Bármely más MCP-kliens (JSON)",
    );
    add(
        "mcp_vscode",
        "Install in VS Code",
        "Nainštalovať vo VS Code",
        "Nainstalovat ve VS Code",
        "In VS Code installieren",
        "Telepítés a VS Code-ba",
    );
    add(
        "mcp_cursor",
        "Install in Cursor",
        "Nainštalovať v Cursore",
        "Nainstalovat v Cursoru",
        "In Cursor installieren",
        "Telepítés a Cursorba",
    );
    add(
        "assistants",
        "Use it in your AI assistant",
        "Použite v AI asistentovi",
        "Použijte v AI asistentovi",
        "In Ihrem KI-Assistenten nutzen",
        "Használja AI-asszisztensében",
    );
    add(
        "assistants_label",
        "Use the data in ChatGPT, Claude or any MCP client",
        "Použite dáta v ChatGPT, Claude alebo v akomkoľvek klientovi MCP",
        "Použijte data v ChatGPT, Claude nebo v libovolném klientovi MCP",
        "Die Daten in ChatGPT, Claude oder jedem MCP-Client nutzen",
        "Használja az adatokat a ChatGPT-ben, a Claude-ban vagy bármely MCP-kliensben",
    );
    add(
        "assistants_lead",
        "Add {name} to the assistant you already use, and ask it about this catalogue's open data. One address covers every dataset; no account is needed.",
        "Pridajte {name} do asistenta, ktorého už používate, a pýtajte sa ho na otvorené dáta tohto katalógu. Jedna adresa pokrýva všetky datasety; účet netreba.",
        "Přidejte {name} do asistenta, kterého už používáte, a ptejte se ho na otevřená data tohoto katalogu. Jedna adresa pokrývá všechny datové sady; účet není potřeba.",
        "Fügen Sie {name} dem Assistenten hinzu, den Sie schon nutzen, und fragen Sie ihn nach den offenen Daten dieses Katalogs. Eine Adresse deckt alle Datensätze ab; kein Konto nötig.",
        "Adja hozzá a(z) {name} csatlakozót a már használt asszisztenséhez, és kérdezze a katalógus nyílt adatairól. Egy cím minden adatkészletet lefed; fiók nem szükséges.",
    );
    add(
        "assistants_sources",
        "What the assistant can search",
        "Čo asistent prehľadáva",
        "Co asistent prohledává",
        "Was der Assistent durchsucht",
        "Miben kereshet az asszisztens",
    );
    add("assistants_sources_lead", "Beside the datasets, the connector searches the public pages and documents of these websites:", "Okrem datasetov konektor prehľadáva verejné stránky a dokumenty týchto webov:", "Kromě datových sad konektor prohledává veřejné stránky a dokumenty těchto webů:", "Neben den Datensätzen durchsucht der Connector die öffentlichen Seiten und Dokumente dieser Websites:", "Az adatkészletek mellett a csatlakozó ezeknek a weboldalaknak a nyilvános oldalain és dokumentumaiban is keres:");
    add(
        "assistants_source_languages",
        "Languages",
        "Jazyky",
        "Jazyky",
        "Sprachen",
        "Nyelvek",
    );
    add(
        "coming_soon",
        "Coming soon",
        "Čoskoro",
        "Brzy",
        "Demnächst",
        "Hamarosan",
    );
    add(
        "connector_status",
        "We are still preparing this connector: it is not yet published in the Claude or ChatGPT directories. Until then, add it as a custom connector by following the guide.",
        "Konektor pripravujeme: v adresároch Claude a ChatGPT ešte nie je zverejnený. Zatiaľ ho pridáte ako vlastný konektor podľa návodu.",
        "Konektor připravujeme: v adresářích Claude a ChatGPT zatím není zveřejněný. Do té doby ho přidáte jako vlastní konektor podle návodu.",
        "Wir bereiten diesen Connector noch vor: In den Verzeichnissen von Claude und ChatGPT ist er noch nicht veröffentlicht. Bis dahin fügen Sie ihn als eigenen Connector nach der Anleitung hinzu.",
        "Ezt a csatlakozót még előkészítjük: a Claude és a ChatGPT könyvtárában még nem jelent meg. Addig adja hozzá egyéni csatlakozóként az útmutató szerint.",
    );
    add(
        "store_address",
        "Each guide shows where to paste this address:",
        "Každý návod ukazuje, kam vložiť túto adresu:",
        "Každý návod ukazuje, kam vložit tuto adresu:",
        "Jede Anleitung zeigt, wo Sie diese Adresse einfügen:",
        "Minden útmutató megmutatja, hová illessze be ezt a címet:",
    );
    add(
        "guide_open",
        "Guide",
        "Návod",
        "Návod",
        "Anleitung",
        "Útmutató",
    );
    add(
        "guide_title",
        "{name}, step by step",
        "{name} krok za krokom",
        "{name} krok za krokem",
        "{name} Schritt für Schritt",
        "{name}, lépésről lépésre",
    );
    add(
        "guide_who",
        "Who can use it",
        "Kto to môže použiť",
        "Kdo to může použít",
        "Wer es nutzen kann",
        "Ki használhatja",
    );
    add(
        "guide_steps",
        "Steps",
        "Postup",
        "Postup",
        "Schritte",
        "Lépések",
    );
    add(
        "guide_try",
        "Try it",
        "Vyskúšajte",
        "Vyzkoušejte",
        "Probieren Sie es aus",
        "Próbálja ki",
    );
    add("connector_note", "Important: the assistant usually won't use the connector on its own. Ask for it in your question, for example:", "Dôležité: asistent konektor väčšinou sám nepoužije. Napíšte mu to v otázke, napríklad:", "Důležité: asistent konektor většinou sám nepoužije. Napište mu to v otázce, například:", "Wichtig: Der Assistent nutzt den Connector meist nicht von selbst. Bitten Sie in Ihrer Frage darum, zum Beispiel:", "Fontos: az asszisztens általában magától nem használja a csatlakozót. Kérje a kérdésében, például:");
    add("connector_why", "The assistant recognises the connector by the name you gave it when you added it.", "Asistent spozná konektor podľa mena, ktoré ste mu dali pri pridaní.", "Asistent pozná konektor podle jména, které jste mu dali při přidání.", "Der Assistent erkennt den Connector an dem Namen, den Sie ihm beim Hinzufügen gegeben haben.", "Az asszisztens arról a névről ismeri fel a csatlakozót, amelyet a hozzáadáskor adott neki.");
    add(
        "guide_use",
        "Use {connector}: ",
        "Použi {connector}: ",
        "Použij {connector}: ",
        "Nutze {connector}: ",
        "Használd a(z) {connector} csatlakozót: ",
    );
    add(
        "guide_example",
        "Which datasets about air quality do you have?",
        "Aké datasety o kvalite ovzdušia máte?",
        "Jaké datové sady o kvalitě ovzduší máte?",
        "Welche Datensätze zur Luftqualität gibt es?",
        "Milyen levegőminőségi adatkészleteid vannak?",
    );
    add(
        "guide_good",
        "A good first answer names records from this catalogue and says which dataset they come from. If the answer is general or from the web, write: use {name}.",
        "Dobrá prvá odpoveď vymenuje záznamy z tohto katalógu a povie, z ktorého datasetu sú. Ak je odpoveď všeobecná alebo z webu, napíšte: použi {name}.",
        "Dobrá první odpověď vyjmenuje záznamy z tohoto katalogu a řekne, ze které datové sady jsou. Pokud je odpověď obecná nebo z webu, napište: použij {name}.",
        "Eine gute erste Antwort nennt Einträge aus diesem Katalog und sagt, aus welchem Datensatz sie stammen. Ist die Antwort allgemein oder aus dem Web, schreiben Sie: nutze {name}.",
        "Egy jó első válasz ebből a katalógusból nevez meg rekordokat, és megmondja, melyik adatkészletből származnak. Ha a válasz általános vagy a webről jön, írja be: használd a(z) {name} csatlakozót.",
    );
    add(
        "guide_trouble",
        "If it does not work",
        "Ak to nejde",
        "Když to nejde",
        "Wenn es nicht klappt",
        "Ha nem működik",
    );
    add(
        "guide_checked",
        "Checked on {date} against {maker}'s own help:",
        "Overené dňa {date} podľa pomocníka {maker}:",
        "Ověřeno dne {date} podle nápovědy {maker}:",
        "Geprüft am {date} anhand der Hilfe von {maker}:",
        "Ellenőrizve {date} napján a(z) {maker} saját súgója alapján:",
    );
    add(
        "guide_back",
        "All assistants",
        "Všetci asistenti",
        "Všichni asistenti",
        "Alle Assistenten",
        "Minden asszisztens",
    );
    add(
        "for_developers",
        "For developers",
        "Pre vývojárov",
        "Pro vývojáře",
        "Für Entwickler",
        "Fejlesztőknek",
    );
    add(
        "any_client",
        "Any MCP client",
        "Akýkoľvek MCP klient",
        "Jakýkoli MCP klient",
        "Jeder MCP-Client",
        "Bármely MCP-kliens",
    );
    add(
        "tagline",
        "Browse them, download them or ask the assistant about them.",
        "Prezrite si ich, stiahnite alebo sa na ne opýtajte asistenta.",
        "Prohlédněte si je, stáhněte nebo se na ně zeptejte asistenta.",
        "Ansehen, herunterladen oder den Assistenten dazu fragen.",
        "Böngéssze, töltse le őket, vagy kérdezze róluk az asszisztenst.",
    );
    add(
        "no_publishers",
        "No publisher matches.",
        "Žiadny vydavateľ nezodpovedá.",
        "Žádný vydavatel neodpovídá.",
        "Kein Herausgeber passt.",
        "Nincs megfelelő kiadó.",
    );
    add(
        "no_datasets",
        "No dataset is published yet.",
        "Zatiaľ nie je zverejnený žiadny dataset.",
        "Zatím není zveřejněna žádná datová sada.",
        "Noch ist kein Datensatz veröffentlicht.",
        "Még nincs közzétett adatkészlet.",
    );
    add(
        "hero_search",
        "Search the open data",
        "Hľadať v otvorených dátach",
        "Hledat v otevřených datech",
        "Offene Daten durchsuchen",
        "Keresés a nyílt adatokban",
    );
    add(
        "hero_placeholder",
        "e.g. air quality, bikes, budget",
        "napr. kvalita ovzdušia, bicykle, rozpočet",
        "např. kvalita ovzduší, kola, rozpočet",
        "z. B. Luftqualität, Fahrräder, Haushalt",
        "pl. levegőminőség, kerékpárok, költségvetés",
    );
    add("search", "Search", "Hľadať", "Hledat", "Suchen", "Keresés");
    add(
        "datasets",
        "datasets",
        "datasetov",
        "datových sad",
        "Datensätze",
        "adatkészlet",
    );
    add(
        "publishers",
        "publishers",
        "vydavateľov",
        "vydavatelů",
        "Herausgeber",
        "kiadó",
    );
    add(
        "recent",
        "Recently updated",
        "Nedávno aktualizované",
        "Nedávno aktualizované",
        "Kürzlich aktualisiert",
        "Nemrég frissítve",
    );
    add(
        "all_datasets",
        "All datasets",
        "Všetky datasety",
        "Všechny datové sady",
        "Alle Datensätze",
        "Összes adatkészlet",
    );
    add(
        "by_publisher",
        "By publisher",
        "Podľa vydavateľa",
        "Podle vydavatele",
        "Nach Herausgeber",
        "Kiadók szerint",
    );
    add(
        "by_keyword",
        "Popular keywords",
        "Časté kľúčové slová",
        "Častá klíčová slova",
        "Häufige Schlagwörter",
        "Népszerű kulcsszavak",
    );
    add(
        "updated",
        "Updated",
        "Aktualizované",
        "Aktualizováno",
        "Aktualisiert",
        "Frissítve",
    );
    add(
        "nav_datasets",
        "Datasets",
        "Datasety",
        "Datové sady",
        "Datensätze",
        "Adatkészletek",
    );
    add(
        "nav_publishers",
        "Publishers",
        "Vydavatelia",
        "Vydavatelé",
        "Herausgeber",
        "Kiadók",
    );
    add("nav_topics", "Topics", "Témy", "Témata", "Themen", "Témák");
    add(
        "nav_about",
        "About",
        "O katalógu",
        "O katalogu",
        "Über den Katalog",
        "Névjegy",
    );
    add(
        "subscribe",
        "Subscribe",
        "Odoberať",
        "Odebírat",
        "Abonnieren",
        "Feliratkozás",
    );
    add(
        "subscribe_calendar",
        "Calendar",
        "Kalendár",
        "Kalendář",
        "Kalender",
        "Naptár",
    );
    add(
        "subscribe_news",
        "News",
        "Novinky",
        "Novinky",
        "Neuigkeiten",
        "Hírek",
    );
    add(
        "subscribe_hint",
        "Copy an address and paste it into Google Calendar, Outlook or a feed reader; new items appear by themselves.",
        "Skopírujte adresu a vložte ju do Google Kalendára, Outlooku alebo čítačky noviniek; nové položky sa objavia samy.",
        "Zkopírujte adresu a vložte ji do Google Kalendáře, Outlooku nebo čtečky novinek; nové položky se objeví samy.",
        "Kopieren Sie eine Adresse und fügen Sie sie in Google Kalender, Outlook oder einen Feedreader ein; neue Einträge erscheinen von selbst.",
        "Másolja ki a címet, és illessze be a Google Naptárba, az Outlookba vagy egy hírolvasóba; az új elemek maguktól megjelennek.",
    );
    add(
        "subscribe_open_app",
        "Open in calendar app",
        "Otvoriť v aplikácii kalendára",
        "Otevřít v aplikaci kalendáře",
        "In der Kalender-App öffnen",
        "Megnyitás naptáralkalmazásban",
    );
    add(
        "subscribe_download",
        "Download .ics",
        "Stiahnuť .ics",
        "Stáhnout .ics",
        ".ics herunterladen",
        ".ics letöltése",
    );
    add(
        "subscribe_open_feed",
        "Open the feed",
        "Otvoriť kanál",
        "Otevřít kanál",
        "Feed öffnen",
        "Hírcsatorna megnyitása",
    );
    add(
        "subscribe_mine",
        "Your own calendar",
        "Váš vlastný kalendár",
        "Váš vlastní kalendář",
        "Ihr eigener Kalender",
        "Saját naptár",
    );
    add(
        "subscribe_mine_lead",
        "Pick a town, a genre or dates: only matching events arrive, and the address below follows your choice.",
        "Vyberte obec, žáner alebo dátumy: prídu len zodpovedajúce podujatia a adresa nižšie sa podľa výberu zmení.",
        "Vyberte obec, žánr nebo data: přijdou jen odpovídající akce a adresa níže se podle výběru změní.",
        "Wählen Sie einen Ort, ein Genre oder Daten: nur passende Termine kommen an, und die Adresse unten folgt Ihrer Wahl.",
        "Válasszon települést, műfajt vagy dátumokat: csak az egyező események érkeznek, és a lenti cím a választást követi.",
    );
    add(
        "subscribe_query_hint",
        "For example {example}; empty for all.",
        "Napríklad {example}; prázdne pre všetky.",
        "Například {example}; prázdné pro všechny.",
        "Zum Beispiel {example}; leer für alle.",
        "Például {example}; üresen az összes.",
    );
    add(
        "subscribe_google",
        "Add to Google Calendar",
        "Pridať do Kalendára Google",
        "Přidat do Kalendáře Google",
        "Zu Google Kalender hinzufügen",
        "Hozzáadás a Google Naptárhoz",
    );
    add(
        "subscribe_refresh",
        "Calendar apps fetch a subscribed calendar again every few hours, so a new event shows up there later than here.",
        "Kalendárové aplikácie načítajú odoberaný kalendár znova každých pár hodín, takže nové podujatie sa v nich objaví neskôr ako tu.",
        "Kalendářové aplikace načtou odebíraný kalendář znovu každých pár hodin, takže nová akce se v nich objeví později než zde.",
        "Kalender-Apps laden einen abonnierten Kalender alle paar Stunden neu, daher erscheint ein neuer Termin dort später als hier.",
        "A naptáralkalmazások néhány óránként töltik be újra a feliratkozott naptárat, így egy új esemény ott később jelenik meg, mint itt.",
    );
    add("subscribe_town", "Town", "Obec", "Obec", "Ort", "Település");
    add(
        "subscribe_genre",
        "Genre",
        "Žáner",
        "Žánr",
        "Genre",
        "Műfaj",
    );
    add("subscribe_from", "From", "Od", "Od", "Von", "Ettől");
    add("subscribe_to", "To", "Do", "Do", "Bis", "Eddig");
    add(
        "subscribe_dates_hint",
        "The day an event starts, for example 2026-12-01; leave both empty for every date.",
        "Deň začiatku podujatia, napríklad 2026-12-01; obe prázdne pre všetky dátumy.",
        "Den začátku akce, například 2026-12-01; obě prázdná pro všechna data.",
        "Der Tag, an dem ein Termin beginnt, zum Beispiel 2026-12-01; beide leer für alle Daten.",
        "Az esemény kezdőnapja, például 2026-12-01; mindkettő üresen az összes dátumhoz.",
    );
    add(
        "subscribe_not_listed",
        "„{value}“ is not in the list; pick one of its values or clear the field.",
        "„{value}“ nie je v zozname; vyberte jednu z jeho hodnôt alebo pole vymažte.",
        "„{value}“ není v seznamu; vyberte jednu z jeho hodnot nebo pole vymažte.",
        "„{value}“ steht nicht in der Liste; wählen Sie einen ihrer Werte oder leeren Sie das Feld.",
        "„{value}” nincs a listában; válasszon egy értéket belőle, vagy törölje a mezőt.",
    );
    add(
        "subscribe_near",
        "Around a town",
        "V okolí obce",
        "V okolí obce",
        "Im Umkreis von",
        "Település környékén",
    );
    add(
        "subscribe_radius",
        "Distance",
        "Vzdialenosť",
        "Vzdálenost",
        "Entfernung",
        "Távolság",
    );
    add(
        "subscribe_near_hint",
        "For example {example}: events within the chosen distance of this place; your device's location is not used.",
        "Napríklad {example}: podujatia do zvolenej vzdialenosti od tejto obce; poloha vášho zariadenia sa nepoužije.",
        "Například {example}: akce do zvolené vzdálenosti od této obce; poloha vašeho zařízení se nepoužije.",
        "Zum Beispiel {example}: Termine im gewählten Umkreis dieses Ortes; der Standort Ihres Geräts wird nicht verwendet.",
        "Például {example}: események a településtől a választott távolságon belül; az eszköz helyzetét nem használjuk.",
    );
    add(
        "subscribe_near_unknown",
        "No event in „{value}“ has a location yet; pick another place or clear the field.",
        "Žiadne podujatie v obci „{value}“ zatiaľ nemá polohu; vyberte iné miesto alebo pole vymažte.",
        "Žádná akce v obci „{value}“ zatím nemá polohu; vyberte jiné místo nebo pole vymažte.",
        "Kein Termin in „{value}“ hat bisher einen Standort; wählen Sie einen anderen Ort oder leeren Sie das Feld.",
        "„{value}” egyetlen eseményének sincs még helye; válasszon másik helyet, vagy törölje a mezőt.",
    );
    add(
        "subscribe_no_facets",
        "This calendar offers no town or genre lists; narrow it by dates, or subscribe to it whole above.",
        "Tento kalendár neponúka zoznam obcí ani žánrov; zúžte ho dátumami alebo ho vyššie odoberajte celý.",
        "Tento kalendář nenabízí seznam obcí ani žánrů; zužte ho daty nebo ho výše odebírejte celý.",
        "Dieser Kalender bietet keine Orts- oder Genrelisten; grenzen Sie ihn mit Daten ein oder abonnieren Sie ihn oben ganz.",
        "Ez a naptár nem kínál település- vagy műfajlistát; szűkítse dátumokkal, vagy fent iratkozzon fel a teljesre.",
    );
    add(
        "subscribe_events",
        "{count} events",
        "{count} podujatí",
        "{count} akcí",
        "{count} Termine",
        "{count} esemény",
    );
    add(
        "subscribe_csv",
        "Table (CSV)",
        "Tabuľka (CSV)",
        "Tabulka (CSV)",
        "Tabelle (CSV)",
        "Táblázat (CSV)",
    );
    add(
        "subscribe_preview",
        "Next 5 events",
        "Najbližších 5 podujatí",
        "Nejbližších 5 akcí",
        "Nächste 5 Termine",
        "A következő 5 esemény",
    );
    add(
        "subscribe_preview_none",
        "No upcoming event matches this query.",
        "Tomuto dopytu nezodpovedá žiadne nadchádzajúce podujatie.",
        "Tomuto dotazu neodpovídá žádná nadcházející akce.",
        "Kein kommender Termin passt zu dieser Abfrage.",
        "Egyetlen közelgő esemény sem felel meg a lekérdezésnek.",
    );
    add(
        "subscribe_preview_loading",
        "Loading the calendar…",
        "Načítavam kalendár…",
        "Načítám kalendář…",
        "Kalender wird geladen…",
        "A naptár betöltése…",
    );
    add(
        "subscribe_preview_failed",
        "The calendar cannot use this choice. The server says: {detail}",
        "Kalendár tento výber nevie použiť. Server hlási: {detail}",
        "Kalendář tento výběr neumí použít. Server hlásí: {detail}",
        "Der Kalender kann diese Auswahl nicht verwenden. Der Server meldet: {detail}",
        "A naptár nem tudja használni ezt a választást. A szerver üzenete: {detail}",
    );
    add(
        "search_publishers",
        "Search publishers…",
        "Hľadať vydavateľov…",
        "Hledat vydavatele…",
        "Herausgeber suchen…",
        "Kiadók keresése…",
    );
    add(
        "search_topics",
        "Search topics…",
        "Hľadať témy…",
        "Hledat témata…",
        "Themen suchen…",
        "Témák keresése…",
    );
    add(
        "try_it",
        "Try it",
        "Vyskúšajte",
        "Vyzkoušejte",
        "Probieren Sie es aus",
        "Próbálja ki",
    );
    add(
        "publishers_help",
        "The cities, regions and offices whose data this catalogue carries. Open one to see all of its datasets.",
        "Mestá, kraje a úrady, ktorých dáta tento katalóg obsahuje. Otvorte jedného a uvidíte všetky jeho datasety.",
        "Města, kraje a úřady, jejichž data tento katalog obsahuje. Otevřete jednoho a uvidíte všechny jeho datové sady.",
        "Die Städte, Regionen und Ämter, deren Daten dieser Katalog enthält. Öffnen Sie einen, um alle seine Datensätze zu sehen.",
        "Azok a városok, régiók és hivatalok, amelyek adatait ez a katalógus tartalmazza. Nyisson meg egyet, és lássa az összes adatkészletét.",
    );
    add(
        "topics_help",
        "A topic gathers the datasets on one subject from every publisher.",
        "Téma spája datasety o jednej veci od všetkých vydavateľov.",
        "Téma spojuje datové sady o jedné věci od všech vydavatelů.",
        "Ein Thema bündelt die Datensätze zu einem Gegenstand von allen Herausgebern.",
        "Egy téma minden kiadótól összegyűjti az egy tárgykörbe tartozó adatkészleteket.",
    );
    add(
        "about_intro_org",
        "{name} is the open data catalogue of {org}.",
        "{name} je katalóg otvorených dát: {org}.",
        "{name} je katalog otevřených dat: {org}.",
        "{name} ist der Katalog offener Daten von {org}.",
        "A(z) {name} a(z) {org} nyílt adatkatalógusa.",
    );
    add(
        "about_intro",
        "{name} is an open data catalogue.",
        "{name} je katalóg otvorených dát.",
        "{name} je katalog otevřených dat.",
        "{name} ist ein Katalog offener Daten.",
        "A(z) {name} egy nyílt adatkatalógus.",
    );
    add(
        "about_data",
        "Every dataset comes straight from the platform's live data. Open one to browse and search its rows, download it as files, or query it live through its API.",
        "Každý dataset pochádza priamo zo živých dát platformy. Otvorte ho a prezerajte či vyhľadávajte v jeho riadkoch, stiahnite si ho ako súbory alebo sa naň pýtajte naživo cez jeho API.",
        "Každá datová sada pochází přímo z živých dat platformy. Otevřete ji a procházejte či vyhledávejte v jejích řádcích, stáhněte si ji jako soubory nebo se na ni ptejte živě přes její API.",
        "Jeder Datensatz stammt direkt aus den Live-Daten der Plattform. Öffnen Sie ihn, um seine Zeilen zu durchsuchen, ihn als Dateien herunterzuladen oder ihn live über seine API abzufragen.",
        "Minden adatkészlet közvetlenül a platform élő adataiból származik. Nyisson meg egyet, és böngéssze, keresse a sorait, töltse le fájlokként, vagy kérdezze le élőben az API-ján keresztül.",
    );
    add(
        "about_machines",
        "For developers and other catalogues",
        "Pre vývojárov a iné katalógy",
        "Pro vývojáře a jiné katalogy",
        "Für Entwickler und andere Kataloge",
        "Fejlesztőknek és más katalógusoknak",
    );
    add(
        "about_api",
        "Every dataset and its description as JSON, through CKAN's Action API.",
        "Všetky datasety a ich opis ako JSON, cez Action API CKAN.",
        "Všechny datové sady a jejich popis jako JSON, přes Action API CKAN.",
        "Alle Datensätze und ihre Beschreibung als JSON, über die Action-API von CKAN.",
        "Minden adatkészlet és leírása JSON formátumban, a CKAN Action API-ján keresztül.",
    );
    add(
        "about_dcat",
        "DCAT-AP catalogue",
        "Katalóg DCAT-AP",
        "Katalog DCAT-AP",
        "DCAT-AP-Katalog",
        "DCAT-AP katalógus",
    );
    add("about_dcat_what", "The whole catalogue as DCAT-AP, for harvesting by national and European data portals.", "Celý katalóg vo formáte DCAT-AP, na zber národnými a európskymi dátovými portálmi.", "Celý katalog ve formátu DCAT-AP, pro sklízení národními a evropskými datovými portály.", "Der ganze Katalog als DCAT-AP, zum Einsammeln durch nationale und europäische Datenportale.", "Az egész katalógus DCAT-AP formátumban, a nemzeti és európai adatportálok általi begyűjtéshez.");
    add(
        "about_contact",
        "Questions about the data",
        "Otázky k dátam",
        "Dotazy k datům",
        "Fragen zu den Daten",
        "Kérdések az adatokról",
    );
    add(
        "more",
        "More details",
        "Ďalšie údaje",
        "Další údaje",
        "Weitere Angaben",
        "További részletek",
    );
    add(
        "copy_address",
        "Copy address",
        "Kopírovať adresu",
        "Kopírovat adresu",
        "Adresse kopieren",
        "Cím másolása",
    );
    add(
        "full_address",
        "The whole address",
        "Celá adresa",
        "Celá adresa",
        "Die ganze Adresse",
        "A teljes cím",
    );
    add(
        "entity_type",
        "Data type",
        "Typ dát",
        "Typ dat",
        "Datentyp",
        "Adattípus",
    );
    add(
        "scope_dataset",
        "Only this dataset's records.",
        "Iba záznamy tohto datasetu.",
        "Pouze záznamy této datové sady.",
        "Nur die Einträge dieses Datensatzes.",
        "Csak ennek az adatkészletnek a rekordjai.",
    );
    add("scope_type", "Every {type} record of the publisher's open data, this dataset's among them.", "Všetky záznamy typu {type} v otvorených dátach vydavateľa, vrátane tohto datasetu.", "Všechny záznamy typu {type} v otevřených datech vydavatele, včetně této datové sady.", "Alle {type}-Einträge der offenen Daten des Herausgebers, auch die dieses Datensatzes.", "A kiadó nyílt adatainak minden {type} rekordja, köztük ennek az adatkészletnek a rekordjai.");
    add(
        "preview_types",
        "The kinds of entities it holds, live",
        "Druhy entít, ktoré obsahuje, naživo",
        "Druhy entit, které obsahuje, živě",
        "Die Arten von Entitäten, live",
        "Milyen entitásokat tartalmaz, élőben",
    );
    add(
        "preview_api",
        "The first records, live",
        "Prvé záznamy naživo",
        "První záznamy živě",
        "Die ersten Einträge, live",
        "Az első rekordok, élőben",
    );
    add(
        "preview_loading",
        "Loading the first records…",
        "Načítavajú sa prvé záznamy…",
        "Načítají se první záznamy…",
        "Die ersten Einträge werden geladen…",
        "Az első rekordok betöltődnek…",
    );
    add(
        "preview_failed",
        "The records did not load here; the address still answers a program.",
        "Záznamy sa tu nenačítali; adresa programu odpovedá aj tak.",
        "Záznamy se zde nenačetly; adresa programu odpovídá i tak.",
        "Die Einträge wurden hier nicht geladen; die Adresse antwortet einem Programm trotzdem.",
        "A rekordok itt nem töltődtek be; a cím egy programnak továbbra is válaszol.",
    );
    add(
        "preview_empty",
        "No records yet.",
        "Zatiaľ žiadne záznamy.",
        "Zatím žádné záznamy.",
        "Noch keine Einträge.",
        "Még nincsenek rekordok.",
    );
    add(
        "preview_shown",
        "{shown} of {total} records",
        "{shown} z {total} záznamov",
        "{shown} z {total} záznamů",
        "{shown} von {total} Einträgen",
        "{shown} / {total} rekord",
    );
    add(
        "records",
        "Records",
        "Počet záznamov",
        "Počet záznamů",
        "Einträge",
        "Rekordok",
    );
    add(
        "ex_first",
        "The first 5 records, with curl",
        "Prvých 5 záznamov cez curl",
        "Prvních 5 záznamů přes curl",
        "Die ersten 5 Einträge mit curl",
        "Az első 5 rekord curl-lel",
    );
    add(
        "ex_file",
        "Download it with curl",
        "Stiahnutie cez curl",
        "Stažení přes curl",
        "Mit curl herunterladen",
        "Letöltés curl-lel",
    );
    add("ex_excel", "In Excel or Power BI: Data, Get Data, From OData Feed, with this address", "V Exceli alebo Power BI: Údaje, Získať údaje, Z informačného kanála OData, s touto adresou", "V Excelu nebo Power BI: Data, Načíst data, Z informačního kanálu OData, s touto adresou", "In Excel oder Power BI: Daten, Daten abrufen, Aus OData-Feed, mit dieser Adresse", "Excelben vagy Power BI-ban: Adatok, Adatok beolvasása, OData-hírcsatornából, ezzel a címmel");
    add("ex_qgis", "In QGIS: Layer, Add Layer, Add WFS / OGC API - Features Layer, a new connection with this address", "V QGIS: Vrstva, Pridať vrstvu, Pridať vrstvu WFS / OGC API - Features, nové pripojenie s touto adresou", "V QGIS: Vrstva, Přidat vrstvu, Přidat vrstvu WFS / OGC API - Features, nové připojení s touto adresou", "In QGIS: Layer, Layer hinzufügen, WFS / OGC API - Features-Layer hinzufügen, eine neue Verbindung mit dieser Adresse", "QGIS-ben: Réteg, Réteg hozzáadása, WFS / OGC API - Features réteg hozzáadása, új kapcsolat ezzel a címmel");
    add("ex_ws", "Changes as they happen, with websocat: open the socket, then send the subscription as the first line", "Zmeny naživo cez websocat: otvorte spojenie a ako prvý riadok pošlite odber", "Změny živě přes websocat: otevřete spojení a jako první řádek pošlete odběr", "Änderungen live mit websocat: Verbindung öffnen, dann das Abonnement als erste Zeile senden", "Változások valós időben websocattal: nyissa meg a socketet, majd első sorként küldje el a feliratkozást");
    add(
        "ex_chatgpt_title",
        "ChatGPT",
        "ChatGPT",
        "ChatGPT",
        "ChatGPT",
        "ChatGPT",
    );
    add("ex_chatgpt", "Open Settings, then Apps and Connectors, and choose Create. Paste this address as the MCP server URL, with no authentication.", "Otvorte Nastavenia, potom Aplikácie a konektory, a zvoľte Vytvoriť. Vložte túto adresu ako URL MCP servera, bez overenia.", "Otevřete Nastavení, pak Aplikace a konektory, a zvolte Vytvořit. Vložte tuto adresu jako URL MCP serveru, bez ověření.", "Öffnen Sie Einstellungen, dann Apps und Konnektoren, und wählen Sie Erstellen. Fügen Sie diese Adresse als URL des MCP-Servers ein, ohne Authentifizierung.", "Nyissa meg a Beállítások, majd az Alkalmazások és csatlakozók menüt, és válassza a Létrehozás lehetőséget. Illessze be ezt a címet MCP-szerver URL-ként, hitelesítés nélkül.");
    add(
        "created",
        "Created",
        "Vytvorené",
        "Vytvořeno",
        "Erstellt",
        "Létrehozva",
    );
    add(
        "info",
        "About this resource",
        "O tomto zdroji",
        "O tomto zdroji",
        "Über diese Ressource",
        "Az erőforrásról",
    );
    add(
        "original_table",
        "The publisher's original table",
        "Pôvodná tabuľka vydavateľa",
        "Původní tabulka vydavatele",
        "Die Originaltabelle des Herausgebers",
        "A kiadó eredeti táblázata",
    );
    add(
        "records_csv",
        "All records as a table",
        "Všetky záznamy ako tabuľka",
        "Všechny záznamy jako tabulka",
        "Alle Einträge als Tabelle",
        "Minden rekord táblázatként",
    );
    add(
        "records_geojson",
        "All records for a map",
        "Všetky záznamy na mapu",
        "Všechny záznamy na mapu",
        "Alle Einträge für eine Karte",
        "Minden rekord térképhez",
    );
    add(
        "records_json",
        "All records for programs",
        "Všetky záznamy pre programy",
        "Všechny záznamy pro programy",
        "Alle Einträge für Programme",
        "Minden rekord programokhoz",
    );
    add(
        "records_xlsx",
        "All records for Excel",
        "Všetky záznamy pre Excel",
        "Všechny záznamy pro Excel",
        "Alle Einträge für Excel",
        "Minden rekord Excelhez",
    );
    add(
        "records_parquet",
        "All records for pandas, Polars or DuckDB",
        "Všetky záznamy pre pandas, Polars alebo DuckDB",
        "Všechny záznamy pro pandas, Polars nebo DuckDB",
        "Alle Einträge für pandas, Polars oder DuckDB",
        "Minden rekord pandashoz, Polarshoz vagy DuckDB-hez",
    );
    add(
        "live_changes",
        "Live changes",
        "Zmeny naživo",
        "Změny živě",
        "Änderungen live",
        "Élő változások",
    );
    add(
        "ex_types",
        "The kinds of entities it holds, with curl",
        "Druhy entít, ktoré obsahuje, cez curl",
        "Druhy entit, které obsahuje, přes curl",
        "Die Arten von Entitäten, die sie enthält, mit curl",
        "Milyen entitásokat tartalmaz, curl-lel",
    );

    // CKAN core words shown on the live pages (`live/*.html`):
    add(
        "skip_to_content",
        "Skip to main content",
        "Skip to main content",
        "Přeskočit na hlavní obsah",
        "Zum Hauptinhalt springen",
        "Ugrás a fő tartalomra",
    );
    add(
        "login",
        "Log in",
        "Prihlásiť sa",
        "Přihlásit se",
        "Anmelden",
        "Bejelentkezés",
    );
    add("home", "Home", "Domov", "Domů", "Startseite", "Kezdőlap");
    add(
        "welcome",
        "Welcome",
        "Vitajte",
        "Vítejte",
        "Willkommen",
        "Üdvözöljük",
    );
    add(
        "search_datasets_placeholder",
        "Search datasets...",
        "Vyhľadať datasety...",
        "Vyhledat datové sady...",
        "Datensätze durchsuchen...",
        "Adatkészletek keresése...",
    );
    add(
        "order_by",
        "Order by",
        "Zoradiť podľa",
        "Seřadit podle",
        "Sortieren nach",
        "Rendezés",
    );
    add(
        "sort_relevance",
        "Relevance",
        "Relevantnosť",
        "Relevance",
        "Relevanz",
        "Relevancia",
    );
    add(
        "sort_name_asc",
        "Name Ascending",
        "Meno vzostupne",
        "Název vzestupně",
        "Name aufsteigend",
        "Név szerint növekvő",
    );
    add(
        "sort_name_desc",
        "Name Descending",
        "Meno zostupne",
        "Název sestupně",
        "Name absteigend",
        "Név szerint csökkenő",
    );
    add(
        "sort_modified",
        "Last Modified",
        "Naposledy zmenené",
        "Naposledy změněno",
        "Zuletzt geändert",
        "Utoljára módosítva",
    );
    add("go", "Go", "Ísť", "Jít", "Los", "Mehet");
    add(
        "api_access",
        "You can also access this registry using the",
        "Prístup do tohto zoznamu je možný aj cez API rozhranie",
        "K tomuto seznamu lze přistupovat také přes",
        "Sie können auf dieses Verzeichnis auch über die",
        "Ezt a jegyzéket elérheti a következőn keresztül is:",
    );
    add(
        "api_docs",
        "see API Docs",
        "viď. dokumentácia API",
        "viz dokumentace API",
        "siehe API-Dokumentation",
        "lásd az API dokumentációt",
    );
    add(
        "api_docs_link",
        "API Docs",
        "Dokumenty API",
        "Dokumentace API",
        "API-Dokumentation",
        "API dokumentáció",
    );
    add(
        "filter_results",
        "Filter Results",
        "Výsledky filtrovania",
        "Filtrovat výsledky",
        "Ergebnisse filtern",
        "Eredmények szűrése",
    );
    add(
        "show_more",
        "Show More",
        "Zobraziť viac",
        "Zobrazit více",
        "Mehr anzeigen",
        "Több megjelenítése",
    );
    add(
        "datasets_found",
        "{n} datasets found",
        "{n} datasety nájdené",
        "{n} datových sad nalezeno",
        "{n} Datensätze gefunden",
        "{n} adatkészlet található",
    );
    add(
        "datasets_found_query",
        "{n} datasets found for \"{q}\"",
        "{n} datasety nájdené na dopyt \"{q}\"",
        "{n} datových sad nalezeno pro dotaz \"{q}\"",
        "{n} Datensätze gefunden für \"{q}\"",
        "{n} adatkészlet található erre: \"{q}\"",
    );
    add(
        "resources_nav",
        "Resources",
        "Zdroje",
        "Zdroje",
        "Ressourcen",
        "Erőforrások",
    );
    add(
        "remove",
        "Remove",
        "Zmazať",
        "Odstranit",
        "Entfernen",
        "Eltávolítás",
    );
    add(
        "search_topics",
        "Search topics...",
        "Hľadať témy...",
        "Hledat témata...",
        "Themen suchen...",
        "Témák keresése...",
    );
    add(
        "no_topics_found",
        "No topics found",
        "Nenájdené žiadne témy",
        "Nenalezena žádná témata",
        "Keine Themen gefunden",
        "Nincs találat a témák között",
    );
    add(
        "empty_topics",
        "There are currently no topics in this catalogue.",
        "V tomto katalógu momentálne nie sú žiadne témy.",
        "V tomto katalogu nyní nejsou žádná témata.",
        "In diesem Katalog gibt es derzeit keine Themen.",
        "Ebben a katalógusban jelenleg nincs téma.",
    );
    add(
        "organizations_found",
        "{n} organizations found",
        "{n} organizácie nájdené",
        "{n} organizací nalezeno",
        "{n} Organisationen gefunden",
        "{n} szervezet található",
    );
    add(
        "view_org",
        "View {name}",
        "Zobraziť {name}",
        "Zobrazit {name}",
        "{name} anzeigen",
        "{name} megtekintése",
    );
    add(
        "read_more",
        "read more",
        "dozvedieť sa viac",
        "číst více",
        "mehr lesen",
        "tovább",
    );
    add(
        "followers",
        "Followers",
        "Odoberatelia",
        "Sledující",
        "Follower",
        "Követők",
    );
    add(
        "members",
        "Members",
        "Členovia",
        "Členové",
        "Mitglieder",
        "Tagok",
    );
    add(
        "about_project",
        "About",
        "O projekte",
        "O projektu",
        "Über",
        "Névjegy",
    );
    add(
        "tags_label",
        "Tags",
        "Tagy",
        "Štítky",
        "Schlagwörter",
        "Címkék",
    );
    add(
        "formats_label",
        "Formats",
        "Formáty",
        "Formáty",
        "Formate",
        "Formátumok",
    );
    add(
        "licences_label",
        "Licences",
        "Licencie",
        "Licence",
        "Lizenzen",
        "Licencek",
    );
    add(
        "dataset_singular",
        "Dataset",
        "Dataset",
        "Datová sada",
        "Datensatz",
        "Adatkészlet",
    );
    add(
        "shacl_validation",
        "SHACL Profile Validation",
        "SHACL validácia profilov",
        "SHACL validace profilů",
        "SHACL-Profilvalidierung",
        "SHACL profilvalidálás",
    );

    m
}

/// Looks up `key` in `lang`, falling back to `en` (`jc_t` in `plugin.py`).
pub fn jc_t(key: &str, lang: &str) -> String {
    let code = normalize_lang_code(lang);
    let table = string_table();
    if let Some(row) = table.get(key) {
        if let Some(v) = row.get(code.as_str()).or_else(|| row.get("en")) {
            return (*v).to_owned();
        }
    }
    key.to_owned()
}

/// Formats an integer with thin-space (`\u{202f}`) thousands separator (`jc_number` in `plugin.py`).
pub fn jc_number(value: usize) -> String {
    let s = value.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.insert(0, '\u{202f}');
        }
        out.insert(0, ch);
    }
    out
}

/// Formats a counted noun in `lang` (`jc_rows` in `plugin.py` lines 2080-2089):
/// `"rows"`, `"datasets_n"` or `"topics_found"`.
pub fn jc_rows(value: usize, key: &str, lang: &str) -> String {
    let number = jc_number(value);
    let code = normalize_lang_code(lang);
    let forms: (&str, &str, &str) = match (key, code.as_str()) {
        ("datasets_n", "sk") => ("dataset", "datasety", "datasetov"),
        ("datasets_n", "cs") => ("datová sada", "datové sady", "datových sad"),
        ("datasets_n", "de") => ("Datensatz", "Datensätze", "Datensätze"),
        ("datasets_n", "hu") => ("adatkészlet", "adatkészlet", "adatkészlet"),
        ("datasets_n", _) => ("dataset", "datasets", "datasets"),
        ("topics_found", "sk") => ("téma nájdená", "témy nájdené", "tém nájdených"),
        ("topics_found", "cs") => ("nalezené téma", "nalezená témata", "nalezených témat"),
        ("topics_found", "de") => ("Thema gefunden", "Themen gefunden", "Themen gefunden"),
        ("topics_found", "hu") => ("téma található", "téma található", "téma található"),
        ("topics_found", _) => ("topic found", "topics found", "topics found"),
        (_, "sk") => ("riadok", "riadky", "riadkov"),
        (_, "cs") => ("řádek", "řádky", "řádků"),
        (_, "de") => ("Zeile", "Zeilen", "Zeilen"),
        (_, "hu") => ("sor", "sor", "sor"),
        (_, _) => ("row", "rows", "rows"),
    };
    let word = if value == 1 {
        forms.0
    } else if (2..=4).contains(&value) {
        forms.1
    } else {
        forms.2
    };
    format!("{number} {word}")
}

/// Formats a date `(year, month, day)` in the page's language matching CKAN's `render_datetime`
/// on `live/*.html` (e.g. `"7. novembra 2026"` in `sk`, `"7. listopadu 2026"` in `cs`, `"November 7, 2026"` in `en`).
pub fn format_date_localized(year: i32, month: u32, day: u32, lang: &str) -> String {
    let code = normalize_lang_code(lang);
    match code.as_str() {
        "sk" => {
            let m = match month {
                1 => "januára",
                2 => "februára",
                3 => "marca",
                4 => "apríla",
                5 => "mája",
                6 => "júna",
                7 => "júla",
                8 => "augusta",
                9 => "septembra",
                10 => "októbra",
                11 => "novembra",
                12 => "decembra",
                _ => "",
            };
            format!("{day}. {m} {year}")
        }
        "cs" => {
            let m = match month {
                1 => "ledna",
                2 => "února",
                3 => "března",
                4 => "dubna",
                5 => "května",
                6 => "června",
                7 => "července",
                8 => "srpna",
                9 => "září",
                10 => "října",
                11 => "listopadu",
                12 => "prosince",
                _ => "",
            };
            format!("{day}. {m} {year}")
        }
        "hu" => {
            let m = match month {
                1 => "január",
                2 => "február",
                3 => "március",
                4 => "április",
                5 => "május",
                6 => "június",
                7 => "július",
                8 => "augusztus",
                9 => "szeptember",
                10 => "október",
                11 => "november",
                12 => "december",
                _ => "",
            };
            format!("{year}. {m} {day}.")
        }
        "de" => {
            let m = match month {
                1 => "Januar",
                2 => "Februar",
                3 => "März",
                4 => "April",
                5 => "Mai",
                6 => "Juni",
                7 => "Juli",
                8 => "August",
                9 => "September",
                10 => "Oktober",
                11 => "November",
                12 => "Dezember",
                _ => "",
            };
            format!("{day}. {m} {year}")
        }
        _ => {
            let m = match month {
                1 => "January",
                2 => "February",
                3 => "March",
                4 => "April",
                5 => "May",
                6 => "June",
                7 => "July",
                8 => "August",
                9 => "September",
                10 => "October",
                11 => "November",
                12 => "December",
                _ => "",
            };
            format!("{m} {day}, {year}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c1_offered_languages_narrows_unknown_locale_without_failing() {
        let offered = vec![
            "sk".into(),
            "en".into(),
            "fi".into(),
            "klingon".into(),
            "hu".into(),
            "sk".into(),
        ];
        let avail = available_languages(&offered);
        assert_eq!(avail, vec!["sk", "en", "hu"]);
    }

    #[test]
    fn c2_language_choice_follows_url_prefix_then_cookie_then_accept_language_then_default() {
        let offered = vec!["sk".into(), "cs".into(), "en".into(), "hu".into()];

        // 1. URL prefix /{lang}/ wins over cookie and Accept-Language
        let r1 = resolve_request_language(
            "/sk/dataset/air-quality",
            Some("ckan_lang=en"),
            Some("cs,en;q=0.8"),
            &offered,
            "en",
        );
        assert_eq!(
            r1,
            LanguageResolution::Resolved {
                lang: "sk".into(),
                stripped_path: "/dataset/air-quality".into(),
                had_url_prefix: true,
            }
        );

        // 2. Cookie wins over Accept-Language and default when no URL prefix is present
        let r2 = resolve_request_language(
            "/dataset/air-quality",
            Some("other=1; ckan_lang=hu"),
            Some("cs,en;q=0.8"),
            &offered,
            "sk",
        );
        assert_eq!(
            r2,
            LanguageResolution::Resolved {
                lang: "hu".into(),
                stripped_path: "/dataset/air-quality".into(),
                had_url_prefix: false,
            }
        );

        // 3. Accept-Language wins over default when no URL prefix or cookie is present
        let r3 = resolve_request_language(
            "/dataset/air-quality",
            None,
            Some("cs-CZ,cs;q=0.9,en;q=0.8"),
            &offered,
            "sk",
        );
        assert_eq!(
            r3,
            LanguageResolution::Resolved {
                lang: "cs".into(),
                stripped_path: "/dataset/air-quality".into(),
                had_url_prefix: false,
            }
        );

        // 4. Catalogue default when none of the above match
        let r4 = resolve_request_language("/dataset/air-quality", None, None, &offered, "sk");
        assert_eq!(
            r4,
            LanguageResolution::Resolved {
                lang: "sk".into(),
                stripped_path: "/dataset/air-quality".into(),
                had_url_prefix: false,
            }
        );

        // 5. Unknown 2-letter prefix is 404 (`UnknownUrlPrefix`)
        let r5 = resolve_request_language("/xx/dataset/air-quality", None, None, &offered, "sk");
        assert_eq!(r5, LanguageResolution::UnknownUrlPrefix("xx".into()));
    }

    #[test]
    fn c3_every_key_exists_in_every_shipped_language_and_templates_hold_no_literal_english() {
        let table = string_table();
        assert!(table.len() >= 110);
        for (key, row) in &table {
            for lang in ["sk", "cs", "en", "hu"] {
                let val = row.get(lang).copied().unwrap_or("");
                assert!(
                    !val.is_empty(),
                    "string key '{key}' is missing a translation for shipped language '{lang}'"
                );
            }
        }

        // Verify that no template in crates/dataroom/web/templates/ holds a literal user-facing English string
        for (tpl_name, tpl_src) in [
            ("home.html", include_str!("../templates/home.html")),
            ("dataset.html", include_str!("../templates/dataset.html")),
        ] {
            for forbidden_literal in [
                "Skip to main content",
                "Search datasets",
                "About this dataset",
                "Recently updated",
                "Browse by topic",
                "Popular keywords",
                "Connect an AI assistant",
                "Data and downloads",
            ] {
                assert!(
                    !tpl_src.contains(forbidden_literal),
                    "template {tpl_name} contains literal English string '{forbidden_literal}'"
                );
            }
        }
    }
}
