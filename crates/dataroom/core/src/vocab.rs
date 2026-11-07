#![forbid(unsafe_code)]
//! Controlled vocabularies from EU Authority Tables, SPDX, IANA, RÚIAN and SK registries.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Standard CKAN and EU licence mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LicenceEntry {
    pub id: String,
    pub code: String,
    pub title: String,
    pub url: String,
    pub eu_iri: String,
    pub is_open: bool,
    pub domain_data: bool,
    pub domain_content: bool,
    pub domain_software: bool,
    pub status: String,
}

/// Returns the built-in licence register (matching CKAN `LicenseRegister` + EU Authority Table).
pub fn default_licence_register() -> Vec<LicenceEntry> {
    vec![
        LicenceEntry {
            id: "cc-by".into(),
            code: "CC_BY_4_0".into(),
            title: "Creative Commons Attribution 4.0".into(),
            url: "https://creativecommons.org/licenses/by/4.0/".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0".into(),
            is_open: true,
            domain_data: true,
            domain_content: true,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "cc-by-sa".into(),
            code: "CC_BYSA_4_0".into(),
            title: "Creative Commons Attribution-ShareAlike 4.0".into(),
            url: "https://creativecommons.org/licenses/by-sa/4.0/".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0".into(),
            is_open: true,
            domain_data: true,
            domain_content: true,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "cc-zero".into(),
            code: "CC0".into(),
            title: "Creative Commons Zero 1.0".into(),
            url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/CC0".into(),
            is_open: true,
            domain_data: true,
            domain_content: true,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "odc-by".into(),
            code: "ODC_BY".into(),
            title: "Open Data Commons Attribution License".into(),
            url: "http://www.opendefinition.org/licenses/odc-by".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/ODC_BY".into(),
            is_open: true,
            domain_data: true,
            domain_content: false,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "odc-odbl".into(),
            code: "ODC_ODBL".into(),
            title: "Open Data Commons Open Database License".into(),
            url: "http://www.opendefinition.org/licenses/odc-odbl".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/ODC_ODBL".into(),
            is_open: true,
            domain_data: true,
            domain_content: false,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "odc-pddl".into(),
            code: "ODC_PDDL".into(),
            title: "Open Data Commons Public Domain Dedication and License".into(),
            url: "http://www.opendefinition.org/licenses/odc-pddl".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/ODC_PDDL".into(),
            is_open: true,
            domain_data: true,
            domain_content: false,
            domain_software: false,
            status: "active".into(),
        },
        LicenceEntry {
            id: "notspecified".into(),
            code: "OP_DATPRO".into(),
            title: "License not specified".into(),
            url: "".into(),
            eu_iri: "http://publications.europa.eu/resource/authority/licence/OP_DATPRO".into(),
            is_open: false,
            domain_data: false,
            domain_content: false,
            domain_software: false,
            status: "active".into(),
        },
    ]
}

/// Resolves a licence by CKAN id, EU code, URL or EU IRI.
pub fn find_licence(input: &str) -> Option<LicenceEntry> {
    let trimmed = input.trim();
    default_licence_register().into_iter().find(|l| {
        l.id.eq_ignore_ascii_case(trimmed)
            || l.code.eq_ignore_ascii_case(trimmed)
            || (!l.url.is_empty() && l.url == trimmed)
            || l.eu_iri == trimmed
    })
}

/// EU Data Themes (`http://publications.europa.eu/resource/authority/data-theme/`).
pub const EU_DATA_THEMES: [(&str, &str, &str, &str); 13] = [
    (
        "AGRI",
        "Agriculture, fisheries, forestry and food",
        "Poľnohospodárstvo, rybárstvo, lesníctvo a potraviny",
        "Zemědělství, rybářství, lesnictví a potraviny",
    ),
    (
        "ECON",
        "Economy and finance",
        "Hospodárstvo a financie",
        "Hospodářství a finance",
    ),
    (
        "EDUC",
        "Education, culture and sport",
        "Vzdelávanie, kultúra a šport",
        "Vzdělávání, kultura a sport",
    ),
    ("ENER", "Energy", "Energetika", "Energie"),
    (
        "ENVI",
        "Environment",
        "Životné prostredie",
        "Životní prostředí",
    ),
    (
        "GOVE",
        "Government and public sector",
        "Vláda a verejný sektor",
        "Vláda a veřejný sektor",
    ),
    ("HEAL", "Health", "Zdravie", "Zdraví"),
    (
        "INTR",
        "International issues",
        "Medzinárodné otázky",
        "Mezinárodní otázky",
    ),
    (
        "JUST",
        "Justice, legal system and public safety",
        "Spravodlivosť, právny systém a verejná bezpečnosť",
        "Spravedlnost, právní systém a veřejná bezpečnost",
    ),
    (
        "REGI",
        "Regions and cities",
        "Regióny a mestá",
        "Regiony a města",
    ),
    (
        "SOCI",
        "Population and society",
        "Obyvateľstvo a spoločnosť",
        "Obyvatelstvo a společnost",
    ),
    (
        "TECH",
        "Science and technology",
        "Veda a technika",
        "Věda a technika",
    ),
    ("TRAN", "Transport", "Doprava", "Doprava"),
];

/// Normalizes a theme code or IRI into the full EU data-theme IRI.
pub fn theme_to_iri(theme: &str) -> String {
    let trimmed = theme.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!(
            "http://publications.europa.eu/resource/authority/data-theme/{}",
            trimmed.to_ascii_uppercase()
        )
    }
}

/// Extracts the short code (`TRAN`, `ENVI`, ...) from a theme IRI or code.
pub fn theme_code(theme: &str) -> String {
    theme
        .trim_end_matches('/')
        .rsplit(['/', '#'])
        .next()
        .unwrap_or(theme)
        .to_owned()
}

/// Label for an EU data theme code in `lang` (`sk`, `cs`, or `en` fallback).
pub fn theme_label(code: &str, lang: &str) -> String {
    let short = theme_code(code);
    for (c, en, sk, cs) in EU_DATA_THEMES {
        if c.eq_ignore_ascii_case(&short) {
            return match lang {
                "sk" => sk.to_owned(),
                "cs" => cs.to_owned(),
                _ => en.to_owned(),
            };
        }
    }
    short
}

/// Normalizes a file format label or IRI into an EU file-type authority IRI.
pub fn format_to_eu_iri(format: &str) -> String {
    let trimmed = format.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_owned();
    }
    let code = match trimmed.to_ascii_uppercase().as_str() {
        "JSON-LD" | "JSONLD" => "JSON_LD",
        "GEOJSON" => "GEOJSON",
        "RDF/XML" | "RDF" => "RDF_XML",
        "TURTLE" | "TTL" => "RDF_TURTLE",
        "N-TRIPLES" | "NT" => "RDF_N_TRIPLES",
        "JSON SCHEMA" => "JSON",
        other => {
            return format!(
                "http://publications.europa.eu/resource/authority/file-type/{}",
                other.replace([' ', '-'], "_")
            )
        }
    };
    format!("http://publications.europa.eu/resource/authority/file-type/{code}")
}

/// Normalizes a media type into an IANA media-type IRI.
pub fn media_type_to_iana_iri(media_type: &str) -> String {
    let trimmed = media_type.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("http://www.iana.org/assignments/media-types/{trimmed}")
    }
}

/// Normalizes an IANA media-type IRI back to `type/subtype`.
pub fn iana_iri_to_media_type(iri: &str) -> String {
    iri.trim()
        .strip_prefix("https://www.iana.org/assignments/media-types/")
        .or_else(|| {
            iri.trim()
                .strip_prefix("http://www.iana.org/assignments/media-types/")
        })
        .unwrap_or(iri.trim())
        .to_owned()
}

/// Normalizes a frequency code or IRI into an EU frequency authority IRI.
pub fn frequency_to_eu_iri(freq: &str) -> String {
    let trimmed = freq.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!(
            "http://publications.europa.eu/resource/authority/frequency/{}",
            trimmed.to_ascii_uppercase()
        )
    }
}

/// Normalizes a 2-letter ISO 639-1 language code into an EU language authority IRI.
pub fn language_to_eu_iri(lang: &str) -> String {
    let trimmed = lang.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_owned();
    }
    let code3 = match trimmed.to_ascii_lowercase().as_str() {
        "sk" => "SLK",
        "cs" | "cz" => "CES",
        "en" => "ENG",
        "fi" => "FIN",
        "ca" => "CAT",
        "de" => "DEU",
        "hu" => "HUN",
        "uk" => "UKR",
        "fr" => "FRA",
        "es" => "SPA",
        other => {
            return format!(
                "http://publications.europa.eu/resource/authority/language/{}",
                other.to_ascii_uppercase()
            )
        }
    };
    format!("http://publications.europa.eu/resource/authority/language/{code3}")
}

/// Normalizes an EU language IRI back to a 2-letter ISO 639-1 language code.
pub fn eu_iri_to_language(iri: &str) -> String {
    let last = iri.trim_end_matches('/').rsplit('/').next().unwrap_or(iri);
    match last.to_ascii_uppercase().as_str() {
        "SLK" => "sk".into(),
        "CES" => "cs".into(),
        "ENG" => "en".into(),
        "FIN" => "fi".into(),
        "CAT" => "ca".into(),
        "DEU" => "de".into(),
        "HUN" => "hu".into(),
        "UKR" => "uk".into(),
        "FRA" => "fr".into(),
        "SPA" => "es".into(),
        _ => last.to_ascii_lowercase(),
    }
}
